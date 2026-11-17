//! One test per attack: each runs in its own process, so a host abort shows up as a signal.

use std::process::Command;

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_luau-spike"))
        .args(args)
        .output()
        .expect("spawn spike");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

macro_rules! attack_tests {
    ($($name:ident),* $(,)?) => {$(
        #[test]
        fn $name() {
            let (ok, text) = run(&["attack", stringify!($name)]);
            assert!(ok, "host did not survive {}: {text}", stringify!($name));
            assert!(text.contains("\"ended\":true") && text.contains("\"vm_usable_after\":true"), "{text}");
        }
    )*};
}

attack_tests!(
    endless_loop,
    endless_loop_in_pcall,
    memory_bomb,
    memory_bomb_in_pcall,
    string_doubling,
    deep_recursion,
    deep_recursion_at_memory_limit,
    pcall_recursion_c_stack,
    error_object_bomb,
    sleeping_loop_forever,
);

#[test]
fn budget_error_names_the_budget() {
    let (_, text) = run(&["attack", "endless_loop"]);
    assert!(text.contains("budget"), "{text}");
}

#[test]
fn process_model_demo() {
    let (ok, text) = run(&["demo"]);
    assert!(ok, "{text}");
}

#[test]
fn reloads_keep_memory_flat() {
    let (ok, text) = run(&["reloads", "300", "bytecode"]);
    assert!(ok, "{text}");
    let growth: i64 = text
        .split("\"rss_growth_kib\":")
        .nth(1)
        .and_then(|s| s.split([',', '}']).next()?.parse().ok())
        .expect("rss_growth_kib");
    assert!(
        growth < 2048,
        "RSS grew {growth} KiB over 200 reloads: {text}"
    );
}
