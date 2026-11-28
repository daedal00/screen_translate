use screen_translate_core::settings::{
    DEFAULT_TARGET, PROVIDERS, TARGETS, is_local_host, is_valid_target, is_valid_url, migrate,
    normalize_provider,
};

fn mig(
    p: Option<&str>,
    u: Option<&str>,
    m: Option<&str>,
    o: Option<&str>,
) -> (String, String, String) {
    migrate(p, u, m, o)
}

fn t(p: &str, u: &str, m: &str) -> (String, String, String) {
    (p.to_string(), u.to_string(), m.to_string())
}

#[test]
fn migrate_missing_or_invalid_provider_is_deepl() {
    assert_eq!(
        mig(None, None, None, None),
        t("deepl", "http://127.0.0.1:8080", "")
    );
    assert_eq!(mig(Some("bogus"), None, None, None).0, "deepl");
}

#[test]
fn migrate_legacy_ollama_without_url() {
    assert_eq!(
        mig(Some("ollama"), None, None, Some("llama3")),
        t("local", "http://127.0.0.1:11434", "llama3")
    );
    assert_eq!(
        mig(Some("ollama"), None, None, None),
        t("local", "http://127.0.0.1:11434", "qwen2.5:7b")
    );
    assert_eq!(
        mig(Some("ollama"), None, Some("m"), Some("llama3")),
        t("local", "http://127.0.0.1:11434", "m")
    );
}

#[test]
fn migrate_local_uses_stored_values_or_defaults() {
    assert_eq!(
        mig(Some("local"), Some("http://10.0.0.5:9000"), Some("m"), None),
        t("local", "http://10.0.0.5:9000", "m")
    );
    assert_eq!(
        mig(Some("local"), None, None, None),
        t("local", "http://127.0.0.1:8080", "")
    );
    assert_eq!(
        mig(Some("local"), Some("http://"), None, None),
        t("local", "http://127.0.0.1:8080", "")
    );
    assert_eq!(
        mig(
            Some("ollama"),
            Some("http://10.0.0.5:9000"),
            None,
            Some("llama3")
        )
        .1,
        "http://10.0.0.5:9000"
    );
    assert_eq!(
        mig(
            Some("ollama"),
            Some("http://10.0.0.5:9000"),
            None,
            Some("llama3")
        )
        .2,
        ""
    );
}

#[test]
fn url_validation() {
    for u in ["http://127.0.0.1:8080", "https://example.com/v1"] {
        assert!(is_valid_url(u), "{u}");
    }
    for u in ["http://", "ftp://x", "", "nonsense"] {
        assert!(!is_valid_url(u), "{u:?}");
    }
}

#[test]
fn local_host_detection() {
    for u in [
        "http://127.0.0.1:8080",
        "http://localhost:11434",
        "http://[::1]:8080",
    ] {
        assert!(is_local_host(u), "{u}");
    }
    for u in [
        "http://example.com",
        "http://192.168.1.2:8080",
        "nonsense",
        "",
    ] {
        assert!(!is_local_host(u), "{u:?}");
    }
}

#[test]
fn default_target_in_targets_and_valid() {
    assert!(TARGETS.contains(&DEFAULT_TARGET));
    assert!(is_valid_target(DEFAULT_TARGET));
}

#[test]
fn targets_include_expected_codes() {
    for c in ["EN-US", "EN-GB", "DE", "FR", "ES", "JA", "ZH-HANS", "KO"] {
        assert!(TARGETS.contains(&c), "missing {c}");
    }
}

#[test]
fn every_target_is_valid() {
    for c in TARGETS {
        assert!(is_valid_target(c), "{c}");
    }
}

#[test]
fn invalid_targets_rejected() {
    for c in ["", "en-us", "XX", " EN-US"] {
        assert!(!is_valid_target(c), "{c:?}");
    }
}

#[test]
fn providers_are_deepl_and_local() {
    assert_eq!(PROVIDERS, &["deepl", "local"]);
}

#[test]
fn normalize_provider_accepts_known() {
    assert_eq!(normalize_provider("deepl"), Some("deepl"));
    assert_eq!(normalize_provider("local"), Some("local"));
}

#[test]
fn normalize_provider_maps_legacy_ollama() {
    assert_eq!(normalize_provider("ollama"), Some("local"));
}

#[test]
fn normalize_provider_rejects_unknown() {
    for p in ["", "DeepL", "x"] {
        assert_eq!(normalize_provider(p), None, "{p:?}");
    }
}
