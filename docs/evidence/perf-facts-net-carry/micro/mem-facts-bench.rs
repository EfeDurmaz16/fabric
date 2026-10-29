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
    async fn scratch_bench_measure_memory() {
        let dir = tempfile::tempdir().unwrap();
        let tools = CountingTools::new(dir.path());
        let home = dir.path().join("home");
        let (before, _) = scratch_footprint();
        let t = std::time::Instant::now();
        for _ in 0..20 {
            std::hint::black_box(NodeFacts::measure(&home, &tools.env, Probes::ALL).await);
        }
        let all = t.elapsed().as_secs_f64();
        let (after, peak) = scratch_footprint();
        println!("BENCH all {all:.6} before {before} after {after} peak {peak}");
    }
}
