use std::time::Duration;
use unbound_protocol::{
    Result,
    glasses::{Connection, Glasses},
};

// Select another Connection recipe to use this same loop with another family.
fn main() -> Result<()> {
    let mut glasses = Glasses::connect(Connection::default(), Duration::from_secs(2))?;
    println!("{:?}: {}", glasses.model(), glasses.serial()?);
    let caps = glasses.capabilities();
    if !caps.sensors {
        return Ok(());
    }
    glasses.start_sensors()?;
    let result = (|| -> Result<()> {
        for _ in 0..100 {
            let report = glasses.next_report()?;
            if let Some(sample) = report.sensors {
                println!(
                    "gyro (rad/s)={:?}, accel (m/s²)={:?}",
                    sample.gyro_rad_s, sample.accel_m_s2
                );
            }
            for event in report.events {
                println!("event: {event:?}");
            }
        }
        Ok(())
    })();
    let stop = if caps.stop_sensors {
        glasses.stop_sensors()
    } else {
        Ok(())
    };
    result?;
    stop
}
