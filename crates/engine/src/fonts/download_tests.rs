//! `font.download` without the network: this crate's tests use a store folder of their own, where
//! downloading was never turned on.

use serde_json::{Value, json};

use super::{Download, catalog, downloads, downloads_allowed, jobs, set, source};
use crate::{EngineError, Session};

#[test]
fn the_store_index_says_which_missing_families_can_be_downloaded() {
    assert_eq!(source("Lato").as_deref(), Some("Google Fonts"));
    assert_eq!(source(" lato ").as_deref(), Some("Google Fonts"), "as the document spells it");
    assert_eq!(source("FigCraft Not A Store Font"), None);

    let mut s = Session::new();
    for family in ["FigCraft Not A Store Font", "Lato"] {
        s.execute("text.create", &json!({"text": "Hamburgefonstiv", "props": {"fontFamily": family}})).unwrap();
    }
    let listed = s.execute("text.missingFonts", &json!({})).unwrap();
    let ours = |name: &str| listed["families"].as_array().unwrap().iter().find(|f| f["family"] == name).cloned();
    assert_eq!(ours("FigCraft Not A Store Font").unwrap()["source"], Value::Null);
    if !figcraft_text::has_family("Lato") {
        assert_eq!(ours("Lato").unwrap()["source"], "Google Fonts");
    }
}

#[test]
fn nothing_downloads_until_the_user_turns_downloading_on() {
    let mut s = Session::new();
    assert!(!downloads_allowed(), "off by default");
    let Some(family) = ["Lato", "Merriweather", "Bungee Shade"].into_iter().find(|f| !figcraft_text::has_family(f)) else { return };
    let refused = s.execute("font.download", &json!({"family": family})).unwrap_err();
    assert!(matches!(&refused, EngineError::Disabled(cmd, why) if cmd == "font.download" && why.contains("turned off")), "{refused}");
    assert!(!downloads().iter().any(|(f, _)| f == family), "no download started");
    assert_eq!(s.execute("font.downloads", &json!({})).unwrap()["allowed"], false);

    assert!(matches!(s.execute("font.download", &json!({})), Err(EngineError::BadParams { .. })));
    let unknown = s.execute("font.download", &json!({"family": "FigCraft Not A Store Font"})).unwrap_err();
    assert!(unknown.to_string().contains("not in the font store's index"), "{unknown}");
}

#[test]
fn a_download_reports_its_progress_then_fits_the_text_once_installed() {
    let family = "FigCraft Download Progress";
    let mut s = Session::new();
    jobs().insert(family.to_lowercase(), (family.into(), Download::Running { done: 50, total: 200 }));
    let r = s.execute("font.download", &json!({"family": family})).unwrap();
    assert_eq!(r, json!({"family": family, "state": "downloading", "done": 50, "total": 200}), "no second download");
    let listed = s.execute("font.downloads", &json!({})).unwrap();
    assert!(listed["downloads"].as_array().unwrap().contains(&json!({"family": family, "state": "downloading", "done": 50, "total": 200})));

    set(family, Download::Failed("downloading x failed".into()));
    let listed = s.execute("font.downloads", &json!({})).unwrap();
    assert!(listed["downloads"].as_array().unwrap().contains(&json!({"family": family, "state": "failed", "error": "downloading x failed"})));

    // Installed (and scanned by the download's thread): the next call fits the text, once.
    set(family, Download::Installed);
    let r = s.execute("font.download", &json!({"family": family})).unwrap();
    assert_eq!(r, json!({"family": family, "state": "installed", "refitted": []}));
    assert!(!downloads().iter().any(|(f, _)| f == family));
}

#[test]
fn the_catalog_offers_store_families_that_arent_installed() {
    let installed: Vec<String> = figcraft_text::family_names().iter().map(|f| f.to_lowercase()).collect();
    let all = catalog("");
    assert!(!all.is_empty(), "the index compiled in has families");
    assert!(all.iter().all(|o| !installed.contains(&o.family.to_lowercase())), "nothing installed is offered");
    assert!(all.iter().all(|o| o.size > 0 && o.source.is_some()), "each offer says what it costs and where it comes from");
    assert!(all.windows(2).all(|w| !w[0].family.eq_ignore_ascii_case(&w[1].family)), "a family is offered once");

    // A search narrows it, ignoring case and the spaces around it.
    let some = catalog("  SANS ");
    assert!(!some.is_empty() && some.len() < all.len());
    assert!(some.iter().all(|o| o.family.to_lowercase().contains("sans")));
    assert!(catalog("FigCraft Not A Store Font").is_empty());

    // The command lists the same, and every family it offers is one font.download knows.
    let listed = Session::new().execute("font.catalog", &json!({"query": "sans"})).unwrap();
    let families = listed["families"].as_array().unwrap();
    assert_eq!(families.len(), some.len());
    assert!(families.iter().all(|f| source(f["family"].as_str().unwrap()).is_some()));
}
