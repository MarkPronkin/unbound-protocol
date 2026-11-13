use unbound_protocol::{
    Result,
    one::{ConnectionOptions, ControlClient, ReportsClient},
};

fn main() -> Result<()> {
    let options = ConnectionOptions::default();
    let mut control = ControlClient::connect(&options)?;
    println!("software: {}", control.software_version()?);
    let mut reports = ReportsClient::connect(&options)?;
    for _ in 0..100 {
        let sample = reports.next_report()?;
        if sample.report_type == 11 {
            println!(
                "{} gyro={:?} accel={:?}",
                sample.timestamp_ns, sample.gyro_rad_s, sample.accel_m_s2
            );
        }
    }
    Ok(())
}
