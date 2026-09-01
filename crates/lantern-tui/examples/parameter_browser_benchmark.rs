fn main() {
    let report = match lantern_tui::benchmark_parameter_browser_20000() {
        Ok(report) => report,
        Err(error) => {
            eprintln!("parameter-browser benchmark failed to execute: {error}");
            std::process::exit(1);
        }
    };

    println!(
        "parameter-browser catalog={} {}x{} frames={} max-window={} p95={}us p99={}us budget=p95<33ms,p99<50ms",
        report.catalog_size,
        report.terminal_width,
        report.terminal_height,
        report.measured_frames,
        report.maximum_virtual_window,
        report.p95.as_micros(),
        report.p99.as_micros(),
    );

    if !report.within_budget() {
        eprintln!(
            "parameter-browser budget exceeded: p95={:?} p99={:?} max-window={}",
            report.p95, report.p99, report.maximum_virtual_window
        );
        std::process::exit(1);
    }
}
