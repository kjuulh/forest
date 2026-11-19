#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Gray,
    Green,
    Red,
    Blue,
    Violet,
    Cyan,
    Pink,
    Orange,
    Yellow,
    Amber,
    Purple,
}

impl Tone {
    pub fn badge_classes(self) -> &'static str {
        match self {
            Tone::Gray => "bg-gray-100 text-gray-700",
            Tone::Green => "bg-green-100 text-green-800",
            Tone::Red => "bg-red-100 text-red-800",
            Tone::Blue => "bg-blue-100 text-blue-800",
            Tone::Violet => "bg-violet-100 text-violet-700",
            Tone::Cyan => "bg-cyan-100 text-cyan-700",
            Tone::Pink => "bg-pink-100 text-pink-800",
            Tone::Orange => "bg-orange-100 text-orange-800",
            Tone::Yellow => "bg-yellow-100 text-yellow-800",
            Tone::Amber => "bg-amber-100 text-amber-700",
            Tone::Purple => "bg-purple-100 text-purple-700",
        }
    }

    pub fn chip_classes(self) -> &'static str {
        match self {
            Tone::Blue => "bg-blue-100 text-blue-700",
            Tone::Red => "bg-red-100 text-red-700",
            Tone::Green => "bg-green-100 text-green-700",
            other => other.badge_classes(),
        }
    }

    pub fn for_environment(env: &str) -> Tone {
        if env.contains("preprod") || env.contains("pre-prod") {
            Tone::Orange
        } else if env.contains("prod") {
            Tone::Pink
        } else if env.contains("stag") {
            Tone::Yellow
        } else if env.contains("dev") {
            Tone::Violet
        } else if env.contains("test") {
            Tone::Cyan
        } else {
            Tone::Gray
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlertKind {
    #[default]
    Error,
    Success,
    Info,
    Warning,
}

impl AlertKind {
    pub fn classes(self) -> &'static str {
        match self {
            AlertKind::Error => "bg-red-50 border-red-200 text-red-700",
            AlertKind::Success => "bg-green-50 border-green-200 text-green-700",
            AlertKind::Info => "bg-blue-50 border-blue-200 text-blue-700",
            AlertKind::Warning => "bg-yellow-50 border-yellow-200 text-yellow-700",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_tones_match_the_jinja_env_badge() {
        assert_eq!(Tone::for_environment("prod"), Tone::Pink);
        assert_eq!(Tone::for_environment("platform-prod"), Tone::Pink);
        assert_eq!(Tone::for_environment("preprod"), Tone::Orange);
        assert_eq!(Tone::for_environment("pre-prod"), Tone::Orange);
        assert_eq!(Tone::for_environment("staging"), Tone::Yellow);
        assert_eq!(Tone::for_environment("platform-dev"), Tone::Violet);
        assert_eq!(Tone::for_environment("test"), Tone::Cyan);
        assert_eq!(Tone::for_environment("sandbox"), Tone::Gray);
    }
}
