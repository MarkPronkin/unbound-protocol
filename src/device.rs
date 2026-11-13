//! Exact USB identities. Recognition is not a claim of hardware validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Air,
    Air2,
    Air2Pro,
    Air2Ultra,
    One,
    OnePro,
    OneS,
    OneProPlus,
    RogR1,
    Light,
    /// Air and Max share a USB identity; inspect the USB product string.
    RokidAirMax,
    GrawoowG530,
    MadGazeGlow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub model: Model,
    pub recovery: bool,
}

pub fn identify(vendor: u16, product: u16) -> Option<Identity> {
    let (model, recovery) = match (vendor, product) {
        (0x3318, 0x0424) => (Model::Air, false),
        (0x3318, 0x0428) => (Model::Air2, false),
        (0x3318, 0x0432) => (Model::Air2Pro, false),
        (0x3318, 0x0426) => (Model::Air2Ultra, false),
        (0x3318, 0x0438) => (Model::One, false),
        (0x3318, 0x0437) => (Model::One, true),
        (0x3318, 0x0436) => (Model::OnePro, false),
        (0x3318, 0x0435) => (Model::OnePro, true),
        (0x3318, 0x043e) => (Model::OneS, false),
        (0x3318, 0x043d) => (Model::OneS, true),
        (0x3318, 0x0444) => (Model::OneProPlus, false),
        (0x0b05, 0x1d9d) => (Model::RogR1, false),
        (0x0b05, 0x1d9c) => (Model::RogR1, true),
        (0x0486, 0x573c) | (0x05a9, 0x0680) => (Model::Light, false),
        (0x04d2, 0x162f) => (Model::RokidAirMax, false),
        (0x1ff7, 0x0ff4) | (0x05a9, 0x0f87) => (Model::GrawoowG530, false),
        (0x04b4, 0x0002) => (Model::MadGazeGlow, false),
        _ => return None,
    };
    Some(Identity { model, recovery })
}

impl Model {
    pub fn is_one(self) -> bool {
        matches!(
            self,
            Self::One | Self::OnePro | Self::OneS | Self::OneProPlus | Self::RogR1
        )
    }
    pub fn is_air(self) -> bool {
        matches!(
            self,
            Self::Air | Self::Air2 | Self::Air2Pro | Self::Air2Ultra
        )
    }
    /// Interface numbers from Air HID evidence. One HID interfaces should be
    /// selected explicitly by path/usage because composite configurations vary.
    pub fn air_interfaces(self) -> Option<(i32, i32)> {
        match self {
            Self::Air2Ultra => Some((0, 2)),
            m if m.is_air() => Some((4, 3)),
            _ => None,
        }
    }
}
