use forage_ui::components::AuthProviders;
use sedge_rt::Render;

#[test]
fn auth_providers_render_only_the_configured_links() {
    let none = AuthProviders {
        google_href: None,
        github_href: None,
        magic_link_href: None,
        magic_link_label: "Sign in with email link",
    }
    .render_to_string();
    assert_eq!(none.trim(), "");

    let some = AuthProviders {
        google_href: None,
        github_href: Some("/auth/github?return_to=%2Fa%22b"),
        magic_link_href: Some("/auth/magic-link"),
        magic_link_label: "Sign up with <email> link",
    }
    .render_to_string();
    assert!(some.contains(">or<"));
    assert!(!some.contains("Continue with Google"));
    assert!(some.contains(r#"href="/auth/github?return_to=%2Fa%22b""#));
    assert!(some.contains("Sign up with &lt;email&gt; link"));
    insta::assert_snapshot!(some);
}
