//! Where does ingest time actually go? read vs hash vs write on 100 MiB.
//!
//! A one-off measurement, kept because it is the refutation record: it
//! shows where the ingest pipeline's milliseconds live, and therefore
//! which optimisations would matter and which would be theatre.
#![allow(missing_docs, clippy::unwrap_used)]
use aperture_cas::hash_bytes;
use std::io::Read;
use std::time::Instant;

fn main() {
    let dir = std::env::temp_dir().join("aperture-ingest-profile");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("blob.bin");
    let data = vec![0x5Au8; 100 * 1024 * 1024];
    std::fs::write(&path, &data).unwrap();

    // Cold-ish read from disk.
    let t = Instant::now();
    let mut file = std::fs::File::open(&path).unwrap();
    let mut buf = Vec::with_capacity(data.len());
    file.read_to_end(&mut buf).unwrap();
    let read = t.elapsed();

    // BLAKE3 hash (the CAS's content address).
    let t = Instant::now();
    let hash = hash_bytes(&buf);
    let hash_t = t.elapsed();

    // Write through the CAS (temp + rename + fsync semantics).
    let t = Instant::now();
    let store = aperture_cas::Store::open(dir.join("objects")).unwrap();
    let (addr, new) = store.put_bytes(&buf).unwrap();
    let write = t.elapsed();

    // Serve path: read the blob back from the CAS.
    let t = Instant::now();
    let back = store.get_bytes(&addr).unwrap();
    let serve = t.elapsed();
    assert_eq!(back.len(), data.len());
    assert!(new);

    let ns = |d: std::time::Duration| d.as_secs_f64() * 1e9;
    let mib = data.len() as f64 / (1024.0 * 1024.0);
    println!("read : {read:?}  ({:.0} MiB/s)", mib / read.as_secs_f64());
    println!(
        "hash : {hash_t:?}  ({:.0} MiB/s)  blake3={}",
        mib / hash_t.as_secs_f64(),
        &hash.hex()[..16]
    );
    println!("write: {write:?}  ({:.0} MiB/s)", mib / write.as_secs_f64());
    println!("serve: {serve:?}  ({:.0} MiB/s)", mib / serve.as_secs_f64());
    let total = read + hash_t + write;
    // io_uring removes syscall *entry* cost (~2 us per op), not the
    // page-cache copies that dominate those millisecond-scale phases. A
    // whole ingest issues ~6 syscalls; the kernel-bypass ceiling is ~12 us
    // of 252 ms — four hundredths of one percent. The copy is the cost, and
    // the CAS must copy (BLAKE3 needs the bytes in userspace to hash them).
    let syscall_share_us = 6.0 * 2.0;
    let total_us = ns(total) / 1e3;
    println!(
        "ingest total: {total:?} ({total_us:.0} us); syscall share io_uring could remove: ~{syscall_share_us:.0} us ({:.4}%)",
        syscall_share_us * 100.0 / total_us
    );
    let _ = std::fs::remove_dir_all(&dir);
}
