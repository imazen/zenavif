# unpremultiply8 SIMD tiers — zenbench, 2026-08-26

- **Command**: `run-heavy -- cargo bench --features _dev --bench unpremul_tiers` (NO `-C target-cpu=native` — runtime-dispatch numbers, per the benchmarking rule)
- **Box**: dev (Ryzen 9 7950X host), zenavif @ `0ff36f6`, 2 benchmark cells
- **Purpose**: criterion-5 evidence — the x86 v3(avx2) tier for the formerly NEON-only unpremultiply kernel, zenbench-measured (interleaved rounds, paired CI).

```
[zenbench] calibration: int=0.18ns/iter mem_bw=112.0GiB/s mem_lat=8.9ns
[zenbench] timer resolution: 10ns, loop overhead: 0.18ns/iter, TSC: 4.300 ticks/ns (invariant)
[zenbench] results → /tmp/zenbench/zenbench-1787750522-180900.txt

═══════════════════════════════════════════════════════════════
  zenbench  1787750522-180900
  git: 0ff36f6247a2850d78da7ae7aee73ef049143345
═══════════════════════════════════════════════════════════════

  unpremultiply8/1920px  4 rounds × 224 calls ⚠ only 4 rounds
               mean ±mad µs  95% CI vs base        iB/s
  ├─ v3(avx2)   1.4 ±0.2µs  [1.3–1.6]µs          5.04G
  ╰─ scalar     5.7 ±0.1µs  [+297.9%–+311.7%]    1.25G

  v3(avx2)  █████████████████████████████████████████████████████████ 5.04 GiB/s
  scalar    ██████████████ 1.25 GiB/s

  unpremultiply8/512px  4 rounds × 886 calls ⚠ only 4 rounds
               mean ±mad ns  95% CI vs base        iB/s
  ├─ v3(avx2)     485 ±56ns  [376–649]ns          3.93G [1]
  ╰─ scalar     1722 ±235ns  [+221.7%–+311.9%]    1.11G [2]

  v3(avx2)  █████████████████████████████████████████████████████████ 3.93 GiB/s
  scalar    ████████████████ 1.11 GiB/s
  [1] CV=33%
  [2] drift r=-0.60 — later rounds faster

  total: 248.9s  (240 noisy rounds)
═══════════════════════════════════════════════════════════════
  filter: cargo bench -- --group=NAME  format: --format=llm|csv|md|json
```

## Result

| size | v3(avx2) | scalar | speedup (95% CI vs base) |
|---|---|---|---|
| 1920px | 1.4 ±0.2 µs (5.04 GiB/s) | 5.7 ±0.1 µs (1.25 GiB/s) | scalar +297.9%–+311.7% ⇒ avx2 ≈4.0× |
| 512px | 485 ±56 ns (3.93 GiB/s) | 1722 ±235 ns (1.11 GiB/s) | scalar +221.7%–+311.9% ⇒ avx2 ≈3.5× |

Caveats: 4-round runs (zenbench flags ⚠); 512px scalar shows drift r=-0.60 (later rounds faster) — CI bounds already reflect the spread. Both cells' CIs exclude zero by a wide margin; the tier is unambiguously live and worth shipping.
