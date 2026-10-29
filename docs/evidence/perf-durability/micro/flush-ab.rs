//! Interleaved A/B of the flush sequences items 3, 12 and 16 change, run
//! in one process on one directory: the anchor's sequence (A) and the
//! candidate's (B) take turns, the first of a pair alternating, so the
//! machine's load reaches both alike. Prints one TSV row per sample.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy)]
enum F { Full, Barrier, Plain }

fn flush(file: &File, how: F) {
    match how {
        F::Full => rustix::fs::fcntl_fullfsync(file).unwrap(),
        F::Barrier => { nix::fcntl::fcntl(file, nix::fcntl::FcntlArg::F_BARRIERFSYNC).unwrap(); }
        F::Plain => rustix::fs::fsync(file).unwrap(),
    }
}
fn flush_dir(dir: &Path, how: F) { flush(&File::open(dir).unwrap(), how) }

/// temp, flush `t`, rename, flush the directory with `d` (None: no flush).
fn atomic(path: &Path, bytes: &[u8], t: F, d: Option<F>) {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp).unwrap();
    f.write_all(bytes).unwrap();
    flush(&f, t);
    fs::rename(&tmp, path).unwrap();
    if let Some(d) = d { flush_dir(path.parent().unwrap(), d) }
}

/// One op in the anchor's way (false) or the candidate's (true).
fn op(name: &str, dir: &Path, candidate: bool, ledger: &mut File) {
    match name {
        // fabricd write_atomic: anchor full+full, candidate barrier+full.
        "write_atomic" => {
            let t = if candidate { F::Barrier } else { F::Full };
            atomic(&dir.join("marker"), &[7u8; 300], t, Some(F::Full));
        }
        // Job::commit: append (write + full) then the manifest.
        "commit" => {
            ledger.write_all(&[1u8; 98]).unwrap();
            flush(ledger, F::Full);
            if candidate {
                atomic(&dir.join("manifest"), &[3u8; 200], F::Barrier, None);
            } else {
                atomic(&dir.join("manifest"), &[3u8; 200], F::Full, Some(F::Full));
            }
        }
        // A capture of 4 files in 2 new directories, then the manifest.
        "capture" => {
            let out = dir.join("out");
            let _ = fs::remove_dir_all(&out);
            fs::create_dir(&out).unwrap();
            let per = if candidate { F::Plain } else { F::Full };
            for sub in ["a", "b"] {
                let d = out.join(sub);
                fs::create_dir(&d).unwrap();
                flush_dir(&out, per);
                for k in 0..2 {
                    if candidate {
                        atomic(&d.join(format!("f{k}")), &[5u8; 4096], F::Plain, Some(F::Plain));
                    } else {
                        atomic(&d.join(format!("f{k}")), &[5u8; 4096], F::Full, Some(F::Full));
                    }
                }
            }
            let t = if candidate { F::Barrier } else { F::Full };
            atomic(&out.join("manifest.json"), &[9u8; 900], t, Some(F::Full));
        }
        // A restore placing 4 files in 2 directories that exist.
        "restore" => {
            let home = dir.join("home");
            for sub in ["a", "b"] {
                let d = home.join(sub);
                fs::create_dir_all(&d).unwrap();
                for k in 0..2 {
                    if candidate {
                        atomic(&d.join(format!("f{k}")), &[6u8; 4096], F::Barrier, None);
                    } else {
                        atomic(&d.join(format!("f{k}")), &[6u8; 4096], F::Full, Some(F::Full));
                    }
                }
            }
            // Each directory once, the last of the device through the cache.
            if candidate {
                flush_dir(&home.join("a"), F::Plain);
                flush_dir(&home.join("b"), F::Full);
            }
        }
        // One flush of a 300-byte write: anchor F_FULLFSYNC, candidate F_BARRIERFSYNC.
        "primitive" => {
            let mut f = OpenOptions::new().create(true).write(true).truncate(true).open(dir.join("p")).unwrap();
            f.write_all(&[2u8; 300]).unwrap();
            flush(&f, if candidate { F::Barrier } else { F::Full });
        }
        _ => unreachable!(),
    }
}

fn main() {
    let dir: PathBuf = std::env::args().nth(1).unwrap().into();
    let pairs: usize = std::env::args().nth(2).unwrap().parse().unwrap();
    fs::create_dir_all(&dir).unwrap();
    let mut ledger = OpenOptions::new().create(true).append(true).open(dir.join("ledger")).unwrap();
    println!("op\tpair\tside\tus");
    let ops: Vec<String> = std::env::args().skip(3).collect();
    for name in ops.iter().map(String::as_str) {
        // Warm up both ways.
        for c in [false, true, false, true] { op(name, &dir, c, &mut ledger); }
        for pair in 0..pairs {
            let order = if pair % 2 == 0 { [false, true] } else { [true, false] };
            for c in order {
                let start = Instant::now();
                op(name, &dir, c, &mut ledger);
                let us = start.elapsed().as_micros();
                println!("{name}\t{pair}\t{}\t{us}", if c { "candidate" } else { "anchor" });
            }
        }
    }
}
