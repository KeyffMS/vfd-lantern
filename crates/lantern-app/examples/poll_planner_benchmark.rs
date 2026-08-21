use std::{error::Error, fmt::Write as _, hint::black_box, time::Duration};

use lantern_app::{
    FrequencyClass, PollCadences, PollPlanner, PollPlannerConfig, ReadSubscription, SubscriberId,
    SubscriptionReason,
};
use lantern_profile::{MAX_PARAMETERS, ProfileFormat, parse_and_validate_profile};

const ITERATIONS: u32 = 10;

fn maximum_profile_json() -> Result<String, std::fmt::Error> {
    let mut source = String::with_capacity(4 * 1024 * 1024);
    source.push_str(
        r#"{"schema_version":1,"profile_id":"benchmark.maximum","revision":1,"vendor":"Benchmark","family":"Planner","model":"Maximum","protocol":{"default_baud_rate":115200,"default_parity":"none","default_data_bits":8,"default_stop_bits":1,"response_timeout_ms":100,"default_slave_id":1,"rs485_mode":"adapter_managed"},"parameters":["#,
    );
    for index in 0..MAX_PARAMETERS {
        if index > 0 {
            source.push(',');
        }
        write!(
            source,
            r#"{{"id":"p{index:05}","code":"P{index:05}","name":"P{index:05}","table":"holding_registers","address":{{"notation":"pdu_zero_based","value":{index}}},"encoding":"unsigned16","quantity":"frequency","unit":"hz"}}"#,
        )?;
    }
    source.push_str("]}");
    Ok(source)
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = maximum_profile_json()?;
    let parse_started = std::time::Instant::now();
    let profile = parse_and_validate_profile(source.as_bytes(), ProfileFormat::Json)?;
    let parse_elapsed = parse_started.elapsed();
    let subscriber = SubscriberId::parse("maximum-profile-benchmark")?;
    let subscriptions = profile
        .parameters()
        .keys()
        .cloned()
        .map(|parameter_id| {
            ReadSubscription::new(
                parameter_id,
                FrequencyClass::Slow,
                subscriber.clone(),
                SubscriptionReason::Diagnostics,
                false,
                Duration::from_secs(3_600),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let cadence = Duration::from_secs(3_600);
    let config = PollPlannerConfig::new(
        PollCadences::new(cadence, cadence, cadence)?,
        profile.protocol().default_link(),
        Duration::ZERO,
        Duration::ZERO,
        700_000,
    )?;
    let planner = PollPlanner::new();
    let now = std::time::Instant::now();
    let planning_started = std::time::Instant::now();
    let mut last_plan = None;
    for _ in 0..ITERATIONS {
        last_plan = Some(planner.build(&profile, subscriptions.clone(), config, now)?);
    }
    let planning_elapsed = planning_started.elapsed();
    let plan = last_plan.ok_or("benchmark did not execute")?;
    black_box(&plan);

    println!("parameters={MAX_PARAMETERS}");
    println!("profile_bytes={}", source.len());
    println!("parse_ms={:.3}", parse_elapsed.as_secs_f64() * 1_000.0);
    println!("iterations={ITERATIONS}");
    println!(
        "planning_total_ms={:.3}",
        planning_elapsed.as_secs_f64() * 1_000.0
    );
    println!(
        "planning_average_ms={:.3}",
        planning_elapsed.as_secs_f64() * 1_000.0 / f64::from(ITERATIONS)
    );
    println!("blocks={}", plan.blocks().len());
    println!("utilization_ppm={}", plan.utilization_ppm());
    println!("rejections={}", plan.rejections().len());
    Ok(())
}
