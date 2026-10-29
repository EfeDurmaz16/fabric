
    #[tokio::test]
    #[ignore = "scratch bench"]
    async fn scratch_bench_round_trips() {
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
        let t = std::time::Instant::now();
        for _ in 0..3000 {
            session.ping(deadline).await.unwrap();
        }
        let ping = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        for _ in 0..3000 {
            let _ = session.append(token, small.clone(), deadline).await;
        }
        let append = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        for _ in 0..60 {
            let _ = session.append(token, big.clone(), deadline).await;
        }
        let big_append = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        for _ in 0..200 {
            let mut set = tokio::task::JoinSet::new();
            for _ in 0..16 {
                let (session, small) = (Arc::clone(&session), small.clone());
                set.spawn(async move { let _ = session.append(token, small, deadline).await; });
            }
            while set.join_next().await.is_some() {}
        }
        let parallel = t.elapsed().as_secs_f64();
        println!("BENCH ping {ping:.6} append {append:.6} big {big_append:.6} parallel {parallel:.6}");
    }
