//! Résultats du garde-fou figés contre une implémentation indépendante.
//!
//! Pourquoi ce module existe
//! Un changement qui modifie ce que le garde-fou voit dans un script doit apparaître comme un échec,
//! pas comme un glissement silencieux. Chaque entrée figée contient les constats et les avertissements
//! qu'une implémentation indépendante des mêmes contrôles rapporte pour le script, et le garde-fou est
//! tenu au même résultat. Les entrées marquées `same_as_reference: false` consignent l'unique
//! différence voulue : un appel `rmdir` ou `unlink` n'est refusé que sur un objet reconnu comme un chemin.

use std::fs;
use std::path::PathBuf;

use libremodular_mcp::security::{GuardPolicy, guard_script};
use serde_json::Value;

fn golden() -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/guard_golden.json");
    let text = fs::read_to_string(path).expect("golden file");
    serde_json::from_str(&text).expect("valid JSON")
}

#[test]
fn every_pinned_script_gives_exactly_the_pinned_findings_and_warnings() {
    let entries = golden();
    assert!(entries.len() >= 90, "only {} pinned scripts", entries.len());
    let mut mismatches = Vec::new();
    for entry in &entries {
        let name = entry["name"].as_str().expect("name");
        let script = entry["script"].as_str().expect("script");
        let policy = GuardPolicy {
            work_dir: "/run/user/1000/libremodular-mcp/work/gimp",
            extra_write_roots: &[],
            home_dir: Some("/home/alice"),
        };
        let report = guard_script(script, &policy);
        let mut findings: Vec<Value> = report
            .findings
            .iter()
            .map(|f| serde_json::json!([f.line, f.category.id(), f.construct]))
            .collect();
        findings.sort_by_key(ToString::to_string);
        let mut pinned = entry["findings"].as_array().expect("findings").clone();
        pinned.sort_by_key(ToString::to_string);
        let mut warnings = report.warnings.clone();
        warnings.sort();
        let mut pinned_warnings: Vec<String> = entry["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .map(|w| w.as_str().expect("warning").to_owned())
            .collect();
        pinned_warnings.sort();
        if findings != pinned || warnings != pinned_warnings {
            mismatches.push(format!(
                "{name}: got {findings:?} {warnings:?}, pinned {pinned:?} {pinned_warnings:?}"
            ));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn only_the_documented_difference_from_the_reference_exists() {
    let different: Vec<String> = golden()
        .iter()
        .filter(|entry| entry["same_as_reference"] == Value::Bool(false))
        .map(|entry| entry["name"].as_str().expect("name").to_owned())
        .collect();
    assert_eq!(
        different,
        [
            "extra_call_result_attr",
            "extra_rmdir_variants",
            "table_004"
        ],
        "an entry now differs from the reference for another reason"
    );
    for entry in golden() {
        if entry["same_as_reference"] == Value::Bool(false) {
            let script = entry["script"].as_str().expect("script");
            assert!(script.contains("rmdir"), "{}", entry["name"]);
        }
    }
}
