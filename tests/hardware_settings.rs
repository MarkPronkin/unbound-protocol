//! Explicit opt-in reversible setting tests. Never writes EDID, input modes,
//! brightness, dimming, scene mode, USB configuration, or firmware.
use std::{fs, path::PathBuf};
use unbound_protocol::{
    Error, Result,
    one::{ConnectionOptions, ControlClient},
};

struct SavedSettings {
    options: ConnectionOptions,
    serial: String,
    proximity: bool,
    stabilization: bool,
    armed: bool,
    journal: PathBuf,
}
impl SavedSettings {
    fn capture() -> std::result::Result<(Self, ControlClient), Box<dyn std::error::Error>> {
        if std::env::var("UNBOUND_ALLOW_SETTINGS_TEST").as_deref() != Ok("1") {
            return Err(
                "set UNBOUND_ALLOW_SETTINGS_TEST=1 to explicitly enable reversible writes".into(),
            );
        }
        let mut options = ConnectionOptions::default();
        if let Ok(source) = std::env::var("UNBOUND_SOURCE") {
            options.source = Some(source.parse()?);
        }
        let mut client = ControlClient::connect(&options)?;
        let serial = client.serial()?;
        if serial.is_empty() {
            return Err("cannot identify device for state restoration".into());
        }
        let proximity = client.proximity_enabled()?;
        let stabilization = client.stabilization_enabled()?;
        fs::create_dir_all("target/hardware")?;
        let journal = PathBuf::from("target/hardware/settings-restoration.json");
        // Written before the first setter so the baseline survives process termination.
        fs::write(
            &journal,
            format!(
                "{{\"restored\":false,\"proximity\":{proximity},\"stabilization\":{stabilization}}}\n"
            ),
        )?;
        Ok((
            Self {
                options,
                serial,
                proximity,
                stabilization,
                armed: true,
                journal,
            },
            client,
        ))
    }
    fn restore(&mut self) -> Result<()> {
        let mut client = ControlClient::connect(&self.options)?;
        if client.serial()? != self.serial {
            return Err(Error::Invalid(
                "different device; refusing to restore another device's settings",
            ));
        }
        if client.proximity_enabled()? != self.proximity {
            client.set_proximity_enabled(self.proximity)?;
        }
        if client.stabilization_enabled()? != self.stabilization {
            client.set_stabilization_enabled(self.stabilization)?;
        }
        if client.proximity_enabled()? != self.proximity
            || client.stabilization_enabled()? != self.stabilization
        {
            return Err(Error::Invalid("original settings did not restore"));
        }
        self.armed = false;
        fs::write(
            &self.journal,
            format!(
                "{{\"restored\":true,\"proximity\":{},\"stabilization\":{}}}\n",
                self.proximity, self.stabilization
            ),
        )?;
        Ok(())
    }
}
impl Drop for SavedSettings {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = self.restore() {
                eprintln!(
                    "SETTING RESTORATION FAILED: {error}; baseline is in {}",
                    self.journal.display()
                );
            }
        }
    }
}

#[test]
#[ignore = "requires UNBOUND_ALLOW_SETTINGS_TEST=1; temporarily toggles proximity and stabilization"]
fn live_one_boolean_setters_with_restoration() -> std::result::Result<(), Box<dyn std::error::Error>>
{
    let (mut saved, mut client) = SavedSettings::capture()?;
    let result = (|| -> Result<()> {
        for value in [false, true] {
            client.set_proximity_enabled(value)?;
            if client.proximity_enabled()? != value {
                return Err(Error::Invalid("proximity setter did not take effect"));
            }
            client.set_stabilization_enabled(value)?;
            if client.stabilization_enabled()? != value {
                return Err(Error::Invalid("stabilization setter did not take effect"));
            }
        }
        Ok(())
    })();
    drop(client);
    let restored = saved.restore();
    // Restoration happens before returning any failed assertion/error.
    restored?;
    result?;
    println!(
        "boolean_setters proximity_false_true=ok stabilization_false_true=ok original_settings_restored=true"
    );
    Ok(())
}

#[test]
#[ignore = "requires UNBOUND_ALLOW_SETTINGS_TEST=1; tests cleanup after an injected operation failure"]
fn live_one_restoration_after_injected_error() -> std::result::Result<(), Box<dyn std::error::Error>>
{
    let (mut saved, mut client) = SavedSettings::capture()?;
    let result = (|| -> Result<()> {
        client.set_proximity_enabled(!saved.proximity)?;
        client.set_stabilization_enabled(!saved.stabilization)?;
        if client.proximity_enabled()? == saved.proximity
            || client.stabilization_enabled()? == saved.stabilization
        {
            return Err(Error::Invalid("temporary settings did not take effect"));
        }
        Err(Error::Invalid("injected operation failure"))
    })();
    drop(client);
    saved.restore()?;
    assert!(matches!(
        result,
        Err(Error::Invalid("injected operation failure"))
    ));
    println!(
        "injected_setting_failure operation_failed_as_expected=true original_settings_restored=true"
    );
    Ok(())
}
