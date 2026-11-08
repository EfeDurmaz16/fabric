//! Shared by the fixture pin tests of this crate: the sha256 the
//! `manifest.sha256` files are written with, a walk of a fixture tree and
//! the manifest parser.
//!
//! sha256 is written out here because the workspace has no sha2 crate and
//! a test does not justify one; the known-answer test in
//! `bridge_fixtures.rs` guards it.

#![allow(
    dead_code,
    reason = "each test binary uses the part of this module it needs"
)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::Path;

pub type Fallible<T> = Result<T, Box<dyn Error>>;

const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// FIPS 180-4 SHA-256 of `data`, as lowercase hex.
#[expect(
    clippy::many_single_char_names,
    reason = "the working variables keep their FIPS 180-4 names"
)]
pub fn sha256(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let bits = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for block in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }
    h.iter().fold(String::with_capacity(64), |mut hex, word| {
        let _ = write!(hex, "{word:08x}");
        hex
    })
}

/// Every file under `dir`, as a path relative to `base` with `/` separators.
pub fn files(base: &Path, dir: &Path, out: &mut Vec<String>) -> Fallible<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            files(base, &path, out)?;
        } else {
            let relative = path.strip_prefix(base)?;
            let parts: Vec<String> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(parts.join("/"));
        }
    }
    Ok(())
}

/// The `manifest.sha256` under `root`: path to digest, checked to be
/// `sha256sum` lines sorted by path.
pub fn manifest(root: &Path) -> Fallible<BTreeMap<String, String>> {
    let text = fs::read_to_string(root.join("manifest.sha256"))?;
    let mut entries = BTreeMap::new();
    let mut previous = String::new();
    for line in text.lines() {
        let (digest, path) = line
            .split_once("  ")
            .ok_or_else(|| format!("manifest line without two spaces: {line}"))?;
        assert_eq!(digest.len(), 64, "{line}");
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "{line}"
        );
        assert!(previous.as_str() < path, "manifest not sorted at {path}");
        path.clone_into(&mut previous);
        entries.insert(path.to_owned(), digest.to_owned());
    }
    Ok(entries)
}

/// Every file under `root` matches `manifest.sha256`, none is missing or
/// extra, and none holds a carriage return.
pub fn check_tree_against_manifest(root: &Path) -> Fallible<()> {
    let listed = manifest(root)?;
    let mut found = Vec::new();
    files(root, root, &mut found)?;
    found.retain(|path| path != "manifest.sha256");
    found.sort();
    let listed_paths: Vec<&String> = listed.keys().collect();
    let found_paths: Vec<&String> = found.iter().collect();
    assert_eq!(found_paths, listed_paths, "files and manifest differ");
    for (path, digest) in &listed {
        let bytes = fs::read(root.join(path))?;
        assert_eq!(
            &sha256(&bytes),
            digest,
            "{path} does not match the manifest"
        );
        assert!(
            !bytes.contains(&b'\r'),
            "{path} has a carriage return; fixtures are LF only"
        );
    }
    Ok(())
}
