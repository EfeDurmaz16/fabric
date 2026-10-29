
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "scratch bench"]
    async fn scratch_bench_measure() {
        let dir = tempfile::tempdir().unwrap();
        let tools = CountingTools::new(dir.path());
        let home = dir.path().join("home");
        for _ in 0..2 {
            std::hint::black_box(NodeFacts::measure(&home, &tools.env, Probes::ALL).await);
        }
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(NodeFacts::measure(&home, &tools.env, Probes::ALL).await);
        }
        let all = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(
                NodeFacts::measure(&home, &tools.env, Probes::of(&[Tool::Node, Tool::Python])).await,
            );
        }
        let keys = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(
                NodeFacts::measure(&home, &tools.env, Probes::of(&[Tool::ICloud])).await,
            );
        }
        let icloud = t.elapsed().as_secs_f64();
        println!("BENCH all {all:.6} keys {keys:.6} icloud {icloud:.6}");
    }
