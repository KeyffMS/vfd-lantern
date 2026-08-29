fn main() {
    let report = match lantern_tui::benchmark_scope_render_120x40() {
        Ok(report) => report,
        Err(error) => {
            eprintln!("scope-render benchmark failed to execute: {error}");
            std::process::exit(1);
        }
    };

    println!(
        "scope-render {}x{} channels={} panels={} points/channel={} frames={} p95={}us p99={}us budget=p95<20ms,p99<33ms",
        report.terminal_width,
        report.terminal_height,
        report.channels,
        report.panels,
        report.points_per_channel,
        report.measured_frames,
        report.p95.as_micros(),
        report.p99.as_micros(),
    );

    if !report.within_budget() {
        eprintln!(
            "scope-render budget exceeded: p95={:?} p99={:?}",
            report.p95, report.p99
        );
        std::process::exit(1);
    }
}
