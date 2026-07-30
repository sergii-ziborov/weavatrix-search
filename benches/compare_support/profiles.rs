use super::{
    ContentVisitControl, Path, Scanner, Workload, benchmark_scan_options, black_box, median,
    millis, ripgrep, ripgrep_count, ripgrep_files, timed, timed_ripgrep, timed_weavatrix,
    weavatrix, weavatrix_count, weavatrix_files,
};

pub(super) fn scan_only(root: &Path) -> u64 {
    Scanner::new(root)
        .options(benchmark_scan_options().with_skip_hidden(true))
        .visit_content_streaming(|_| |_| ContentVisitControl::Continue)
        .expect("scan-only content visit")
        .completed
}

pub(super) fn profile(root: &Path, workload: &Workload, runs: usize, warmups: usize) {
    let expected = weavatrix(root, workload.query.clone(), workload.search_mode).1;
    let rg_expected = ripgrep(
        root,
        workload.patterns,
        workload.fixed,
        workload.search_mode,
    )
    .1;
    assert_eq!(
        expected, rg_expected,
        "{} output differs from ripgrep",
        workload.mode
    );

    for _ in 0..warmups {
        black_box(weavatrix(
            root,
            workload.query.clone(),
            workload.search_mode,
        ));
        black_box(ripgrep(
            root,
            workload.patterns,
            workload.fixed,
            workload.search_mode,
        ));
    }

    let mut ours = Vec::with_capacity(runs);
    let mut rg = Vec::with_capacity(runs);
    let mut files = 0_u64;
    for index in 0..runs {
        if index % 2 == 0 {
            let (elapsed, output, searched) =
                timed_weavatrix(root, workload.query.clone(), workload.search_mode);
            assert_eq!(output, expected);
            files = searched;
            ours.push(elapsed);
            let (elapsed, output) = timed_ripgrep(
                root,
                workload.patterns,
                workload.fixed,
                workload.search_mode,
            );
            assert_eq!(output, expected);
            rg.push(elapsed);
        } else {
            let (elapsed, output) = timed_ripgrep(
                root,
                workload.patterns,
                workload.fixed,
                workload.search_mode,
            );
            assert_eq!(output, expected);
            rg.push(elapsed);
            let (elapsed, output, searched) =
                timed_weavatrix(root, workload.query.clone(), workload.search_mode);
            assert_eq!(output, expected);
            files = searched;
            ours.push(elapsed);
        }
    }
    println!(
        "mode={} engine=weavatrix-search files={files} matching_lines={} median_ms={:.3}",
        workload.mode,
        expected.len(),
        millis(median(&mut ours))
    );
    println!(
        "mode={} engine=ripgrep-cli files={files} matching_lines={} median_ms={:.3}",
        workload.mode,
        expected.len(),
        millis(median(&mut rg))
    );
}

pub(super) fn profile_count(root: &Path, runs: usize, warmups: usize) {
    let expected = weavatrix_count(root);
    assert_eq!(expected, ripgrep_count(root));
    let occurrences = expected.iter().map(|(_, count)| count).sum::<u64>();
    for _ in 0..warmups {
        black_box(weavatrix_count(root));
        black_box(ripgrep_count(root));
    }
    let mut ours = Vec::with_capacity(runs);
    let mut rg = Vec::with_capacity(runs);
    for index in 0..runs {
        if index % 2 == 0 {
            ours.push(timed(|| weavatrix_count(root), &expected));
            rg.push(timed(|| ripgrep_count(root), &expected));
        } else {
            rg.push(timed(|| ripgrep_count(root), &expected));
            ours.push(timed(|| weavatrix_count(root), &expected));
        }
    }
    println!(
        "mode=count engine=weavatrix-search matched_files={} occurrences={} median_ms={:.3}",
        expected.len(),
        occurrences,
        millis(median(&mut ours))
    );
    println!(
        "mode=count engine=ripgrep-cli matched_files={} occurrences={} median_ms={:.3}",
        expected.len(),
        occurrences,
        millis(median(&mut rg))
    );
}

pub(super) fn profile_files(root: &Path, runs: usize, warmups: usize) {
    let expected = weavatrix_files(root);
    assert_eq!(expected, ripgrep_files(root));
    for _ in 0..warmups {
        black_box(weavatrix_files(root));
        black_box(ripgrep_files(root));
    }
    let mut ours = Vec::with_capacity(runs);
    let mut rg = Vec::with_capacity(runs);
    for index in 0..runs {
        if index % 2 == 0 {
            ours.push(timed(|| weavatrix_files(root), &expected));
            rg.push(timed(|| ripgrep_files(root), &expected));
        } else {
            rg.push(timed(|| ripgrep_files(root), &expected));
            ours.push(timed(|| weavatrix_files(root), &expected));
        }
    }
    println!(
        "mode=files engine=weavatrix-search matched_files={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut ours))
    );
    println!(
        "mode=files engine=ripgrep-cli matched_files={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut rg))
    );
}
