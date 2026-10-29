// Appended to crates/fabric-net/src/frame.rs at ceba4ce and run with
// cargo test -p fabric-net --release --lib -- scratch_bench --ignored --nocapture
// old_read_frame is read_frame as it was before ceba4ce.
#[cfg(test)]
mod scratch_bench {
    use super::*;

    async fn old_read_frame<R: AsyncRead + Unpin + ?Sized>(
        reader: &mut R,
        max: usize,
    ) -> Result<Vec<u8>, FrameError> {
        let mut header = [0u8; LENGTH_LEN];
        reader.read_exact(&mut header).await.map_err(FrameError::Io)?;
        let length = u32::from_le_bytes(header);
        let len = usize::try_from(length).unwrap();
        if len > max {
            return Err(FrameError::TooLarge { len, max });
        }
        let mut body = Vec::with_capacity(len.min(INITIAL_CAPACITY));
        let read = reader
            .take(u64::from(length))
            .read_to_end(&mut body)
            .await
            .map_err(FrameError::Io)?;
        if read < len {
            return Err(FrameError::Truncated);
        }
        if body[0] != FRAME_VERSION {
            return Err(FrameError::BadVersion(body[0]));
        }
        body.drain(..1);
        Ok(body)
    }

    #[test]
    #[ignore = "scratch bench"]
    fn bench_read_frame() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        // First run: [100, 8 << 10, (1 << 20) + 70, (8 << 20) + 70] with 31 pairs;
        // second run: [12, 100, 1000] with 201 pairs and 50000 iterations.
        for len in [100usize, 8 << 10, (1 << 20) + 70, (8 << 20) + 70] {
            let payload = vec![7u8; len];
            let mut wire = Vec::new();
            rt.block_on(write_frame(&mut wire, &payload, MAX_BULK_FRAME)).unwrap();
            let iters = if len > 1 << 20 { 40 } else if len > 1 << 16 { 300 } else { 20000 };
            let mut ratios = Vec::new();
            let (mut old_cap, mut new_cap) = (0, 0);
            for pair in 0..31 {
                let mut run = |new: bool| {
                    let t = std::time::Instant::now();
                    for _ in 0..iters {
                        let mut cursor = wire.as_slice();
                        let body = if new {
                            rt.block_on(read_frame(&mut cursor, MAX_BULK_FRAME)).unwrap()
                        } else {
                            rt.block_on(old_read_frame(&mut cursor, MAX_BULK_FRAME)).unwrap()
                        };
                        if new { new_cap = body.capacity() } else { old_cap = body.capacity() }
                        std::hint::black_box(body);
                    }
                    t.elapsed().as_secs_f64()
                };
                let (a, b) = if pair % 2 == 0 {
                    let a = run(false);
                    (a, run(true))
                } else {
                    let b = run(true);
                    (run(false), b)
                };
                ratios.push(b / a);
            }
            ratios.sort_by(f64::total_cmp);
            println!(
                "len {len}: median new/old {:.3} (min {:.3} max {:.3}), capacity old {old_cap} new {new_cap}",
                ratios[ratios.len() / 2],
                ratios[0],
                ratios[ratios.len() - 1]
            );
        }
    }
}
