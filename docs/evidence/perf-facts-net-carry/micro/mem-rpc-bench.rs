    fn scratch_footprint() -> (u64, u64) {
        let out = std::process::Command::new("/usr/bin/footprint")
            .args(["-p", &std::process::id().to_string(), "-f", "bytes", "--noCategories"])
            .output()
            .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        let get = |name: &str| {
            text.lines()
                .find(|l| l.trim_start().starts_with(name))
                .unwrap()
                .trim()
                .trim_start_matches(name)
                .trim()
                .trim_end_matches('B')
                .trim()
                .parse::<u64>()
                .unwrap()
        };
        (get("phys_footprint:"), get("phys_footprint_peak:"))
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "scratch bench"]
    async fn scratch_bench_rpc_memory() {
        use crate::wire::{RecordBytes, RecordPage};
        let (session, _serving) = serving_pair(paired(), Arc::default()).await;
        let session = Arc::new(session);
        let token = FencingToken {
            job_id: fabric_core::id::JobId::from_bytes([1; 16]),
            epoch: Epoch::FIRST,
        };
        let small = RecordPage::new(RecordBytes::new(vec![7; 200]).unwrap());
        let big = RecordPage::new(RecordBytes::new(vec![7; 1 << 20]).unwrap());
        let deadline = Duration::from_secs(10);
        let (before, _) = scratch_footprint();
        let t = std::time::Instant::now();
        for _ in 0..1000 {
            session.ping(deadline).await.unwrap();
        }
        for _ in 0..1000 {
            let _ = session.append(token, small.clone(), deadline).await;
        }
        for _ in 0..40 {
            let _ = session.append(token, big.clone(), deadline).await;
        }
        for _ in 0..100 {
            let mut set = tokio::task::JoinSet::new();
            for _ in 0..16 {
                let (session, small) = (Arc::clone(&session), small.clone());
                set.spawn(async move { let _ = session.append(token, small, deadline).await; });
            }
            while set.join_next().await.is_some() {}
        }
        let all = t.elapsed().as_secs_f64();
        let (after, peak) = scratch_footprint();
        println!("BENCH all {all:.6} before {before} after {after} peak {peak}");
    }
}
