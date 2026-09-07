/// Turn our serializer's poster+sequence output into a pure image sequence,
/// preserving every sample/metadata byte offset. The still-image avif/mif1
/// brands cannot remain after removing the primary meta box.
pub fn remove_poster(data: &mut [u8]) {
    let mut pos = 0;
    let mut found = false;
    while pos < data.len() {
        let size = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && pos + size <= data.len());
        if &data[pos + 4..pos + 8] == b"ftyp" {
            assert_eq!(&data[pos + 8..pos + 12], b"avis");
            let mut brands: Vec<[u8; 4]> = data[pos + 16..pos + size]
                .chunks_exact(4)
                .filter(|b| *b != b"avif" && *b != b"mif1" && *b != b"avis")
                .map(|b| b.try_into().unwrap())
                .collect();
            if !brands.contains(b"msf1") {
                brands.push(*b"msf1");
            }
            let new_size = 16 + brands.len() * 4;
            assert!(new_size <= size && (new_size == size || size - new_size >= 8));
            data[pos..pos + 4].copy_from_slice(&(new_size as u32).to_be_bytes());
            for (index, brand) in brands.iter().enumerate() {
                data[pos + 16 + index * 4..pos + 20 + index * 4].copy_from_slice(brand);
            }
            if new_size < size {
                data[pos + new_size..pos + size].fill(0);
                data[pos + new_size..pos + new_size + 4]
                    .copy_from_slice(&((size - new_size) as u32).to_be_bytes());
                data[pos + new_size + 4..pos + new_size + 8].copy_from_slice(b"free");
            }
        } else if &data[pos + 4..pos + 8] == b"meta" {
            assert!(!found);
            found = true;
            data[pos + 4..pos + 8].copy_from_slice(b"free");
        }
        pos += size;
    }
    assert!(found, "fixture had no poster meta to remove");
}
