/// Phone-only preferences. No session, pairing, audio, or credential data belongs here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Preferences {
    pub speak_results: bool,
    pub haptics: bool,
    pub auto_scan: bool,
    pub english_voice: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            speak_results: true,
            haptics: true,
            auto_scan: false,
            english_voice: false,
        }
    }
}

impl Preferences {
    pub fn set(&mut self, key: &str, value: bool) -> bool {
        match key {
            "speak_results" => self.speak_results = value,
            "haptics" => self.haptics = value,
            "auto_scan" => self.auto_scan = value,
            "english_voice" => self.english_voice = value,
            _ => return false,
        }
        true
    }

    pub fn entries(self) -> [(&'static str, bool); 4] {
        [
            ("speak_results", self.speak_results),
            ("haptics", self.haptics),
            ("auto_scan", self.auto_scan),
            ("english_voice", self.english_voice),
        ]
    }

    pub fn from_values(mut read: impl FnMut(&str) -> Option<String>) -> Self {
        let mut result = Self::default();
        for (key, _) in result.entries() {
            if let Some(value) = read(key).and_then(|value| value.parse::<bool>().ok()) {
                result.set(key, value);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_and_corrupt_values_preserve_defaults() {
        assert_eq!(Preferences::from_values(|_| None), Preferences::default());
        assert_eq!(
            Preferences::from_values(|_| Some("broken".into())),
            Preferences::default()
        );
    }
    #[test]
    fn preferences_roundtrip_without_session_data() {
        let expected = Preferences {
            speak_results: false,
            haptics: false,
            auto_scan: true,
            english_voice: true,
        };
        let actual = Preferences::from_values(|key| {
            expected
                .entries()
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        });
        assert_eq!(actual, expected);
        let mut actual = actual;
        assert!(!actual.set("session_key", true));
        assert_eq!(actual, expected);
    }
}
