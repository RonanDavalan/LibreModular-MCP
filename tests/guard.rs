//! Script guard tests.
//!
//! Why this module exists
//! The guard must refuse the dangerous constructs and stay silent on normal scripts.

#[macro_use]
mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Verdict, WORK_DIR, must_fail, must_pass};
use libremodular_mcp::security::{
    Category, Finding, GuardPolicy, GuardReport, format_refusal, guard_script, run_isolated,
};

type Guard<'g> = &'g dyn Fn(&str, &GuardPolicy<'_>) -> GuardReport;

fn policy(extra: &[String]) -> GuardPolicy<'_> {
    GuardPolicy {
        work_dir: WORK_DIR,
        extra_write_roots: extra,
        home_dir: Some("/home/alice"),
    }
}

fn analyse(script: &str) -> GuardReport {
    guard_script(script, &policy(&[]))
}

fn found(report: &GuardReport) -> BTreeSet<(String, usize)> {
    report
        .findings
        .iter()
        .map(|f| (f.category.id().to_owned(), f.line))
        .collect()
}

type Case = (&'static str, &'static [(&'static str, usize)]);

fn refused_cases() -> Vec<(&'static str, Vec<Case>)> {
    vec![
        (
            "file deletion",
            vec![
                ("import os\nos.remove('/home/alice/a')\n", &[("G-1", 2)]),
                (
                    "import os\nos.unlink(p)\nos.rmdir(p)\nos.removedirs(p)\n",
                    &[("G-1", 2), ("G-1", 3), ("G-1", 4)],
                ),
                ("import shutil\nshutil.rmtree(d)\n", &[("G-1", 2)]),
                (
                    "from pathlib import Path\np = Path('/home/alice/a')\np.unlink()\n",
                    &[("G-1", 3)],
                ),
                (
                    "import pathlib\npathlib.Path('/home/alice/d').rmdir()\n",
                    &[("G-1", 2)],
                ),
                (
                    "import os\nf = os.remove\nf('x')\n",
                    &[("G-1", 2), ("G-1", 3)],
                ),
                ("import os\nlist(map(os.remove, files))\n", &[("G-1", 2)]),
            ],
        ),
        (
            "process launch",
            vec![
                (
                    "import subprocess\nsubprocess.run(['ls'])\n",
                    &[("G-2", 1), ("G-2", 2)],
                ),
                ("import os\nos.system('ls')\n", &[("G-2", 2)]),
                ("import os\nos.popen('ls')\n", &[("G-2", 2)]),
                ("import os\nos.execv('/bin/ls', ['ls'])\n", &[("G-2", 2)]),
                (
                    "import os\nos.spawnl(os.P_WAIT, '/bin/ls', 'ls')\n",
                    &[("G-2", 2)],
                ),
                ("import pty\npty.spawn('sh')\n", &[("G-2", 1)]),
                (
                    "from subprocess import Popen\nPopen(['ls'])\n",
                    &[("G-2", 1), ("G-2", 2)],
                ),
            ],
        ),
        (
            "network access",
            vec![
                (
                    "import socket\ns = socket.socket()\n",
                    &[("G-3", 1), ("G-3", 2)],
                ),
                (
                    "import urllib.request\nurllib.request.urlopen('http://x')\n",
                    &[("G-3", 1), ("G-3", 2)],
                ),
                ("from http.client import HTTPSConnection\n", &[("G-3", 1)]),
                ("from http import client\n", &[("G-3", 1)]),
                ("import requests\n", &[("G-3", 1)]),
                ("import smtplib\n", &[("G-3", 1)]),
                ("import ftplib\n", &[("G-3", 1)]),
            ],
        ),
        (
            "write outside the work directory",
            vec![
                (
                    "with open('/etc/passwd', 'w') as f:\n    f.write('x')\n",
                    &[("G-4", 1)],
                ),
                ("open('/home/alice/o.txt', 'a').write('x')\n", &[("G-4", 1)]),
                ("open('/home/alice/o.bin', mode='wb')\n", &[("G-4", 1)]),
                (
                    "from pathlib import Path\nPath('/home/alice/x').write_text('x')\n",
                    &[("G-4", 2)],
                ),
                (
                    "from pathlib import Path\nPath('/home/alice/x').write_bytes(b'x')\n",
                    &[("G-4", 2)],
                ),
                (
                    "import shutil\nshutil.copy('a', '/home/alice/b')\n",
                    &[("G-4", 2)],
                ),
                (
                    "import shutil\nshutil.copyfile('a', '/home/alice/b')\n",
                    &[("G-4", 2)],
                ),
                (
                    "import shutil\nshutil.move('/home/alice/a', LMM_WORK_DIR)\n",
                    &[("G-4", 2)],
                ),
                ("OUT = '/home/alice/o.txt'\nopen(OUT, 'w')\n", &[("G-4", 2)]),
                (
                    "import os\nopen(os.path.join('/home/alice', 'x'), 'w')\n",
                    &[("G-4", 2)],
                ),
                (
                    "import os\nopen(os.path.join(LMM_WORK_DIR, '..', '..', 'x'), 'w')\n",
                    &[("G-4", 2)],
                ),
                ("open('~/x.txt', 'w')\n", &[("G-4", 1)]),
            ],
        ),
        (
            "analysis bypass",
            vec![
                ("eval('1+1')\n", &[("G-5", 1)]),
                ("exec('x = 1')\n", &[("G-5", 1)]),
                ("compile('1', 'f', 'eval')\n", &[("G-5", 1)]),
                ("__import__('os').getcwd()\n", &[("G-5", 1)]),
                (
                    "import importlib\nimportlib.import_module('os')\n",
                    &[("G-5", 1), ("G-5", 2)],
                ),
                ("import ctypes\n", &[("G-5", 1)]),
                ("import builtins\nbuiltins.eval('1')\n", &[("G-5", 2)]),
                ("getattr(__builtins__, 'eval')('1')\n", &[("G-5", 1)]),
            ],
        ),
        (
            "alias",
            vec![
                ("import os as o\no.remove('x')\n", &[("G-1", 2)]),
                (
                    "from shutil import rmtree as r\nr('x')\n",
                    &[("G-1", 1), ("G-1", 2)],
                ),
                (
                    "from os import system as run\nrun('ls')\n",
                    &[("G-2", 1), ("G-2", 2)],
                ),
                ("import subprocess as sp\nsp.call(['ls'])\n", &[("G-2", 2)]),
                ("from os import *\nremove('x')\n", &[("G-1", 2)]),
                ("import os\ngetattr(os, 'remove')('x')\n", &[("G-1", 2)]),
                (
                    "import shutil as s\nfn = s.rmtree\nfn('x')\n",
                    &[("G-1", 2), ("G-1", 3)],
                ),
            ],
        ),
    ]
}

fn check_refuses_with_right_line(guard: Guard<'_>) -> Verdict {
    for (category, cases) in refused_cases() {
        for (script, expected) in cases {
            let report = guard(script, &policy(&[]));
            let reported = found(&report);
            for (id, line) in expected {
                ensure!(
                    reported.contains(&((*id).to_owned(), *line)),
                    "{category}: {script:?} missed {id} at line {line}"
                );
            }
        }
    }
    Ok(())
}

fn nothing_guard(_script: &str, _policy: &GuardPolicy<'_>) -> GuardReport {
    GuardReport::default()
}

fn text_search_guard(script: &str, _policy: &GuardPolicy<'_>) -> GuardReport {
    let mut report = GuardReport::default();
    for (index, line) in script.lines().enumerate() {
        if line.contains("os.remove") {
            report.findings.push(Finding {
                category: Category::FileDeletion,
                construct: "os.remove".to_owned(),
                line: index + 1,
            });
        }
    }
    report
}

#[test]
fn every_category_is_refused_with_category_and_line() {
    must_pass(check_refuses_with_right_line(&guard_script));
}

#[test]
fn guard_refusal_check_fails_on_defective_guards() {
    must_fail(check_refuses_with_right_line(&nothing_guard));
    must_fail(check_refuses_with_right_line(&text_search_guard));
}

fn check_ignores_strings_and_comments(guard: Guard<'_>) -> Verdict {
    let script = "\
# os.remove(path) would delete the file
message = \"call os.remove(x) or subprocess.run(y)\"
\"\"\"eval(socket) in a docstring\"\"\"
print(\"shutil.rmtree\")
";
    let report = guard(script, &policy(&[]));
    ensure!(report.findings.is_empty(), "{:?}", report.findings);
    Ok(())
}

fn substring_guard(script: &str, _policy: &GuardPolicy<'_>) -> GuardReport {
    let mut report = GuardReport::default();
    for (index, line) in script.lines().enumerate() {
        for needle in ["os.remove", "subprocess.run", "shutil.rmtree", "eval("] {
            if line.contains(needle) {
                report.findings.push(Finding {
                    category: Category::FileDeletion,
                    construct: needle.to_owned(),
                    line: index + 1,
                });
            }
        }
    }
    report
}

#[test]
fn strings_and_comments_are_not_findings() {
    must_pass(check_ignores_strings_and_comments(&guard_script));
    must_fail(check_ignores_strings_and_comments(&substring_guard));
}

#[test]
fn local_names_that_look_like_modules_or_builtins_are_not_findings() {
    let script = "\
import re
pattern = re.compile(\"a+\")
for socket in node.inputs:
    socket.default_value = 0
collection.objects.unlink(obj)
bpy.data.objects.remove(obj)
def compile(x):
    return x
";
    assert!(analyse(script).findings.is_empty());
}

#[test]
fn rmdir_and_unlink_are_refused_only_on_path_objects() {
    assert!(
        analyse("collection.objects.unlink(obj)\nfolder.rmdir()\n")
            .findings
            .is_empty()
    );
    assert!(
        analyse("def clean(entry):\n    entry.unlink()\n    entry.rmdir()\n")
            .findings
            .is_empty()
    );
    let report = analyse("from pathlib import Path\nPath('/home/alice/a').rmdir()\n");
    assert!(found(&report).contains(&("G-1".to_owned(), 2)));
    let report = analyse("import pathlib\nbase = pathlib.Path('/x') / 'y'\nbase.unlink()\n");
    assert!(found(&report).contains(&("G-1".to_owned(), 3)));
}

#[test]
fn invalid_script_gives_an_empty_report_with_a_warning() {
    let report = analyse("def (:\n");
    assert!(report.findings.is_empty());
    assert_eq!(
        report.warnings,
        vec!["syntax error not analysed".to_owned()]
    );
    assert!(analyse("x = 1\0\n").findings.is_empty());
}

#[test]
fn pathological_nesting_is_refused_not_skipped() {
    let script = format!("x = {}1{}\n", "(".repeat(3000), ")".repeat(3000));
    let report = analyse(&script);
    assert!(report.refused(), "{report:?}");
    assert!(report.categories().contains(&Category::AnalysisBypass));
}

#[test]
fn one_hundred_thousand_nested_parentheses_neither_crash_the_process_nor_pass() {
    let balanced = format!("x = {}1{}\n", "(".repeat(100_000), ")".repeat(100_000));
    let unclosed = "(".repeat(100_000);
    let brackets = format!("x = {}", "[".repeat(100_000));
    let braces = format!("x = {}", "{".repeat(100_000));
    let fstrings = format!("x = f\"{}\"", "{f'".repeat(5_000));
    for (name, script) in [
        ("balanced parentheses", balanced),
        ("unclosed parentheses", unclosed),
        ("unclosed brackets", brackets),
        ("unclosed braces", braces),
    ] {
        let report = analyse(&script);
        assert!(report.refused(), "{name} was not refused: {report:?}");
        assert!(
            report.categories().contains(&Category::AnalysisBypass),
            "{name}"
        );
    }
    // An f-string can hide nesting from a naive scanner; whatever the answer is,
    // the process must still be alive to give it.
    let _ = analyse(&fstrings);
}

#[test]
fn nesting_at_the_interpreter_limit_is_still_analysed() {
    let script = format!(
        "x = {}1{}\nimport os\nos.remove('x')\n",
        "(".repeat(150),
        ")".repeat(150)
    );
    assert!(found(&analyse(&script)).contains(&("G-1".to_owned(), 3)));
}

fn repeated(unit: &str, tail: &str, total: usize) -> String {
    format!("x = {}{tail}\n", unit.repeat(total / unit.len()))
}

#[test]
fn hostile_unbracketed_chains_do_not_crash_the_process() {
    let total = 262_144;
    let shapes = [
        repeated("-", "1", total),
        repeated("not ", "1", total),
        repeated("lambda:", "1", total),
        repeated("1+", "1", total),
        repeated("a.", "b", total),
        format!("x = f{}\n", "()".repeat(total / 2)),
        repeated("a**", "a", total),
        repeated("a if b else ", "c", total),
        format!("x = a{}\n", "[0]".repeat(total / 3)),
    ];
    for (index, script) in shapes.iter().enumerate() {
        let started = Instant::now();
        let _ = analyse(script);
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "shape {index} was too slow"
        );
    }
}

#[test]
fn a_script_too_large_for_a_safe_analysis_is_refused_not_attempted() {
    let script = "x = 1\n".repeat(400_000);
    let report = analyse(&script);
    assert!(report.refused(), "{:?}", report.warnings);
    assert!(format_refusal(&report).contains("too large to analyse"));
}

#[test]
fn name_resolution_that_explodes_is_refused_within_a_bounded_time() {
    // Twelve levels of a name assigned twenty times each: every resolution would
    // try twenty to the power of twelve combinations without a work budget.
    let mut script = String::new();
    for level in 0..12 {
        for _ in 0..20 {
            script.push_str(&format!("n{level} = n{}\n", level + 1));
        }
    }
    script.push_str("n0.x()\n");
    let started = Instant::now();
    let report = analyse(&script);
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(report.refused(), "{report:?}");
}

#[test]
fn computed_write_path_is_a_warning_not_a_finding() {
    let report = analyse("open(some_function(), 'w')\n");
    assert!(report.findings.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("not statically verifiable"))
    );
}

#[test]
fn relative_write_path_is_a_warning() {
    let report = analyse("open('out.txt', 'w')\n");
    assert!(report.findings.is_empty() && !report.warnings.is_empty());
}

#[test]
fn reading_outside_the_work_directory_is_allowed() {
    assert!(
        analyse("open('/etc/hostname').read()\n")
            .findings
            .is_empty()
    );
}

#[test]
fn extra_write_roots_are_honoured() {
    let script = "open('/home/alice/exports/a.txt', 'w')\n";
    assert!(!analyse(script).findings.is_empty());
    let extra = vec!["/home/alice/exports".to_owned()];
    assert!(guard_script(script, &policy(&extra)).findings.is_empty());
}

#[test]
fn work_dir_writes_are_allowed() {
    let script = "import os\nopen(os.path.join(LMM_WORK_DIR, 'a.txt'), 'w')\n";
    assert!(analyse(script).findings.is_empty());
}

#[test]
fn a_replaced_work_directory_name_is_followed_not_trusted() {
    let script = "LMM_WORK_DIR = '/home/alice'\nopen(LMM_WORK_DIR + '/x', 'w')\n";
    assert!(found(&analyse(script)).contains(&("G-4".to_owned(), 2)));
}

#[test]
fn a_work_directory_symlink_that_leaves_the_directory_is_a_finding() {
    let base = tempfile::tempdir().expect("temporary directory");
    let work = base.path().join("work");
    let outside = base.path().join("outside");
    fs::create_dir(&work).expect("work");
    fs::create_dir(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, work.join("link")).expect("symlink");
    let work_text = work.to_str().expect("utf-8 path").to_owned();
    let policy = GuardPolicy {
        work_dir: &work_text,
        extra_write_roots: &[],
        home_dir: None,
    };
    let inside = format!("open('{work_text}/ok.txt', 'w')\n");
    let escaping = format!("open('{work_text}/link/x.txt', 'w')\n");
    assert!(guard_script(&inside, &policy).findings.is_empty());
    assert!(!guard_script(&escaping, &policy).findings.is_empty());
}

#[test]
fn refusal_message_names_category_construct_line_and_remedy() {
    let report = analyse("x = 1\nimport os\nos.remove('/home/alice/a')\n");
    let message = format_refusal(&report);
    assert!(message.contains("G-1") && message.contains("line 3") && message.contains("os.remove"));
    assert!(message.contains("allow_dangerous_scripts"));
}

#[test]
fn refusal_message_lists_at_most_ten_findings() {
    let script: String = (0..14)
        .map(|i| format!("import os\nos.remove('/x{i}')\n"))
        .collect();
    let message = format_refusal(&analyse(&script));
    assert!(message.contains("... and "), "{message}");
    assert!(message.lines().filter(|l| l.contains("G-1")).count() <= 10);
}

fn realistic_scripts() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/realistic");
    let mut scripts: Vec<(String, String)> = fs::read_dir(&dir)
        .expect("fixtures directory")
        .map(|entry| {
            let path = entry.expect("fixture entry").path();
            let name = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned();
            (name, fs::read_to_string(&path).expect("fixture text"))
        })
        .collect();
    scripts.sort();
    scripts
}

fn check_no_false_positives(guard: Guard<'_>) -> Verdict {
    for (name, script) in realistic_scripts() {
        let report = guard(&script, &policy(&[]));
        ensure!(report.findings.is_empty(), "{name}: {:?}", report.findings);
    }
    Ok(())
}

#[test]
fn realistic_scripts_are_never_refused_and_false_positive_rate_is_zero() {
    let scripts = realistic_scripts();
    assert!(
        scripts.len() >= 20,
        "only {} realistic scripts",
        scripts.len()
    );
    let refused: Vec<_> = scripts
        .iter()
        .filter(|(_, script)| analyse(script).refused())
        .map(|(name, _)| name.clone())
        .collect();
    let rate = refused.len() as f64 / scripts.len() as f64;
    println!(
        "false positives: {} of {} realistic scripts ({:.1}%)",
        refused.len(),
        scripts.len(),
        rate * 100.0
    );
    assert!(refused.is_empty(), "refused: {refused:?}");
}

fn import_everything_guard(script: &str, _policy: &GuardPolicy<'_>) -> GuardReport {
    let mut report = GuardReport::default();
    if script.contains("import") {
        report.findings.push(Finding {
            category: Category::ProcessLaunch,
            construct: "import".to_owned(),
            line: 1,
        });
    }
    report
}

#[test]
fn false_positive_check_fails_on_an_overzealous_guard() {
    must_pass(check_no_false_positives(&guard_script));
    must_fail(check_no_false_positives(&import_everything_guard));
}

#[test]
fn long_flat_expressions_are_still_analysed() {
    let script = format!(
        "x = {}\nimport os\nos.remove('x')\n",
        vec!["1"; 3000].join("+")
    );
    assert!(found(&analyse(&script)).contains(&("G-1".to_owned(), 3)));
}

#[test]
fn a_two_hundred_fifty_six_kibibyte_ordinary_script_is_analysed_without_error() {
    let mut script = String::new();
    let mut index = 0_u32;
    while script.len() < 256 * 1024 {
        script.push_str(&format!(
            "value_{index} = round(math.sqrt({index}) * 3.5, 2) + len(items_{index})\n"
        ));
        index += 1;
    }
    script.truncate(256 * 1024);
    let end = script.rfind('\n').map_or(0, |i| i + 1);
    script.truncate(end);
    let started = Instant::now();
    let report = analyse(&script);
    assert!(report.findings.is_empty(), "{:?}", report.findings.first());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings.first());
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[test]
fn lines_are_counted_for_every_newline_convention() {
    let script = "x = 1\r\nimport os\r\nos.remove('a')\rimport os\ros.remove('b')\n";
    let lines = found(&analyse(script));
    assert!(lines.contains(&("G-1".to_owned(), 3)), "{lines:?}");
    assert!(lines.contains(&("G-1".to_owned(), 5)), "{lines:?}");
}

#[test]
fn category_identifiers_are_stable_values() {
    let ids: Vec<&str> = [
        Category::FileDeletion,
        Category::ProcessLaunch,
        Category::NetworkAccess,
        Category::WriteOutsideWorkDir,
        Category::AnalysisBypass,
    ]
    .iter()
    .map(|c| c.id())
    .collect();
    assert_eq!(ids, ["G-1", "G-2", "G-3", "G-4", "G-5"]);
}

fn check_a_panic_in_the_analysis_is_a_refusal(isolate: &dyn Fn(usize) -> GuardReport) -> Verdict {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| isolate(100)));
    let Ok(report) = outcome else {
        return Err("the panic escaped the isolation".to_owned());
    };
    ensure!(report.refused(), "a panic was not turned into a refusal");
    ensure!(
        report.categories() == [Category::AnalysisBypass],
        "wrong category for a panic: {:?}",
        report.categories()
    );
    Ok(())
}

fn unprotected(script_len: usize) -> GuardReport {
    let _ = script_len;
    let panicking = || -> GuardReport { panic!("the parser gave up") };
    panicking()
}

#[test]
fn a_panic_of_the_analyser_is_captured_and_refuses_the_script() {
    let panicking = |script_len: usize| {
        run_isolated(script_len, || -> GuardReport {
            panic!("the parser gave up")
        })
    };
    must_pass(check_a_panic_in_the_analysis_is_a_refusal(&panicking));
    must_fail(check_a_panic_in_the_analysis_is_a_refusal(&unprotected));
}

#[test]
fn an_analysis_that_finds_nothing_passes_through_the_isolation_unchanged() {
    let report = run_isolated(10, GuardReport::default);
    assert_eq!(report, GuardReport::default());
}
