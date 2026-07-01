#!/usr/bin/env python3
"""Color-exact RGB<->YCbCr(BT.601 full-range) and RGB planar (identity) I/O for
apples-to-apples aomenc encoding. Matches zenravif's BT.601 full-range math
(ravif/src/animated.rs:540-545): Y=0.299R+0.587G+0.114B (round,clamp 0..255);
Cb=(B-Y)*0.5/(1-0.114)+128; Cr=(R-Y)*0.5/(1-0.299)+128.
Inverse is the exact algebraic inverse (full-range).
Chroma 4:2:0 downsample = box average (matches zenravif accumulate+round).

Feeding aomenc the SAME YUV zenravif would produce is what makes the libaom
comparison an ENCODER comparison, not a color-conversion comparison. A naive
ffmpeg RGB<->YUV round-trip injected ~MAE 0.07 and capped ssim2 ~66; this does not.

Usage:
  color.py to_y4m  IN.png  FMT  OUT.y4m     FMT in {420,444,rgb}
  color.py from_y4m IN.y4m FMT REF.png OUT.png   (REF only for dims/sanity)
"""
import sys, numpy as np
from PIL import Image

BT = np.array([0.2990, 0.5870, 0.1140], dtype=np.float64)

def rgb_to_ycc(rgb):  # rgb uint8 HxWx3 -> Y,Cb,Cr float HxW (pre-round)
    r,g,b = rgb[...,0].astype(np.float64), rgb[...,1].astype(np.float64), rgb[...,2].astype(np.float64)
    y = BT[0]*r + BT[1]*g + BT[2]*b
    cb = (b - y)*0.5/(1.0-BT[2]) + 128.0
    cr = (r - y)*0.5/(1.0-BT[0]) + 128.0
    return y, cb, cr

def ycc_to_rgb(y, cb, cr):  # exact inverse, full range
    y=y.astype(np.float64); cb=cb.astype(np.float64); cr=cr.astype(np.float64)
    b = (cb-128.0)*(1.0-BT[2])/0.5 + y
    r = (cr-128.0)*(1.0-BT[0])/0.5 + y
    g = (y - BT[0]*r - BT[2]*b)/BT[1]
    rgb = np.stack([r,g,b], axis=-1)
    return np.clip(np.round(rgb),0,255).astype(np.uint8)

def box420(plane):  # HxW float -> ceil(H/2)xceil(W/2) uint8, round-half-up box avg
    H,W = plane.shape
    Hc,Wc = (H+1)//2,(W+1)//2
    out = np.zeros((Hc,Wc), dtype=np.float64); cnt=np.zeros((Hc,Wc))
    pr = np.clip(np.round(plane),0,255)
    for dy in (0,1):
        for dx in (0,1):
            sub = pr[dy::2, dx::2]
            out[:sub.shape[0],:sub.shape[1]] += sub
            cnt[:sub.shape[0],:sub.shape[1]] += 1
    return np.clip(np.floor(out/cnt + 0.5),0,255).astype(np.uint8)

def up420(plane, H, W):  # nearest-neighbor upsample chroma to full res
    return np.repeat(np.repeat(plane, 2, axis=0), 2, axis=1)[:H,:W]

def write_y4m(path, planes, W, H, cs):
    with open(path,"wb") as f:
        f.write(f"YUV4MPEG2 W{W} H{H} F25:1 Ip A0:0 {cs} XYSCSS={cs[1:]}\nFRAME\n".encode())
        for p in planes: f.write(p.tobytes())

def read_y4m(path):
    with open(path,"rb") as f:
        data=f.read()
    nl=data.index(b"\n"); hdr=data[:nl].decode()
    W=int([t[1:] for t in hdr.split() if t.startswith("W")][0])
    H=int([t[1:] for t in hdr.split() if t.startswith("H")][0])
    cs=[t for t in hdr.split() if t.startswith("C")][0]
    fs=data.index(b"FRAME"); fe=data.index(b"\n",fs)
    body=data[fe+1:]
    return W,H,cs,body

def main():
    cmd=sys.argv[1]
    if cmd=="to_y4m":
        inp,fmt,out=sys.argv[2],sys.argv[3],sys.argv[4]
        rgb=np.asarray(Image.open(inp).convert("RGB"))
        H,W=rgb.shape[:2]
        if fmt=="rgb":
            g,b,r=rgb[...,1],rgb[...,2],rgb[...,0]
            write_y4m(out,[g,b,r],W,H,"C444")
        elif fmt=="444":
            y,cb,cr=rgb_to_ycc(rgb)
            Y=np.clip(np.round(y),0,255).astype(np.uint8)
            U=np.clip(np.round(cb),0,255).astype(np.uint8)
            V=np.clip(np.round(cr),0,255).astype(np.uint8)
            write_y4m(out,[Y,U,V],W,H,"C444")
        elif fmt=="420":
            y,cb,cr=rgb_to_ycc(rgb)
            Y=np.clip(np.round(y),0,255).astype(np.uint8)
            U=box420(cb); V=box420(cr)
            write_y4m(out,[Y,U,V],W,H,"C420jpeg")
        else: raise SystemExit("bad fmt")
    elif cmd=="from_y4m":
        inp,fmt,ref,out=sys.argv[2],sys.argv[3],sys.argv[4],sys.argv[5]
        W,H,cs,body=read_y4m(inp)
        if fmt=="rgb":
            n=W*H
            g=np.frombuffer(body[:n],np.uint8).reshape(H,W)
            b=np.frombuffer(body[n:2*n],np.uint8).reshape(H,W)
            r=np.frombuffer(body[2*n:3*n],np.uint8).reshape(H,W)
            rgb=np.stack([r,g,b],axis=-1)
        elif fmt=="444":
            n=W*H
            Y=np.frombuffer(body[:n],np.uint8).reshape(H,W)
            U=np.frombuffer(body[n:2*n],np.uint8).reshape(H,W)
            V=np.frombuffer(body[2*n:3*n],np.uint8).reshape(H,W)
            rgb=ycc_to_rgb(Y,U,V)
        elif fmt=="420":
            n=W*H; Hc,Wc=(H+1)//2,(W+1)//2; nc=Hc*Wc
            Y=np.frombuffer(body[:n],np.uint8).reshape(H,W)
            U=np.frombuffer(body[n:n+nc],np.uint8).reshape(Hc,Wc)
            V=np.frombuffer(body[n+nc:n+2*nc],np.uint8).reshape(Hc,Wc)
            Uu=up420(U,H,W); Vv=up420(V,H,W)
            rgb=ycc_to_rgb(Y,Uu,Vv)
        else: raise SystemExit("bad fmt")
        Image.fromarray(rgb,"RGB").save(out)
    else: raise SystemExit("bad cmd")

if __name__=="__main__": main()
