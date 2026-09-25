//! `ui/pattern-syntax.html` is generated from `help/pattern-syntax.md`, which the
//! maintainer writes by hand. Both shells embed `ui/` verbatim — Tauri serves it
//! as `frontendDist` and cha-web `include_dir!`s it — so the page has to exist
//! there as a checked-in file; this test is what keeps it from drifting from its
//! source. Run with `UPDATE_HELP=1` to regenerate instead of failing.

use std::path::Path;

use pulldown_cmark::{html, Options, Parser};

fn render() -> String {
    let help = Path::new(env!("CARGO_MANIFEST_DIR")).join("../help");
    let read = |name: &str| {
        std::fs::read_to_string(help.join(name))
            .unwrap_or_else(|e| panic!("reading help/{name}: {e}"))
            .replace("\r\n", "\n")
    };
    let markdown = read("pattern-syntax.md");
    let template = read("pattern-syntax.template.html");
    assert_eq!(
        template.matches("{{content}}").count(),
        1,
        "the template must contain exactly one {{{{content}}}} marker"
    );

    let mut body = String::new();
    html::push_html(
        &mut body,
        Parser::new_ext(&markdown, Options::ENABLE_TABLES),
    );
    template.replace("{{content}}", &body)
}

#[test]
fn pattern_syntax_page_is_up_to_date() {
    let page = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/pattern-syntax.html");
    let expected = render();
    if std::env::var_os("UPDATE_HELP").is_some() {
        std::fs::write(&page, &expected).expect("writing ui/pattern-syntax.html");
        return;
    }
    let actual = std::fs::read_to_string(&page)
        .expect("reading ui/pattern-syntax.html")
        .replace("\r\n", "\n");
    assert!(
        actual == expected,
        "ui/pattern-syntax.html is stale. Edit help/pattern-syntax.md (never the \
         .html), then regenerate with:\n  UPDATE_HELP=1 cargo test -p cha-gui --test help_page"
    );
}
