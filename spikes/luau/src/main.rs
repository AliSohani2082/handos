//! Spike 4 driver.
//!
//!   luau-spike report [--json out.json]   everything below, each attack in its own process
//!   luau-spike demo                       the process model working end to end
//!   luau-spike attack <name>              one attack; exit 0 means the host survived it
//!   luau-spike reloads <n> <variant>      n reloads; variant: source | bytecode | large-source | large-bytecode
//!
//! Pass bar: no case aborts the host; a reload of the standard library takes 50 ms or less;
//! memory stays flat.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use luau_spike::{Config, Outcome, Runtime, STD_SOURCE, Std, compile_std, padded_std, rss_kib};

pub const ATTACKS: &[(&str, &str)] = &[
    ("endless_loop", "while true do end"),
    (
        "endless_loop_in_pcall",
        "while true do pcall(function() while true do end end) end",
    ),
    (
        "memory_bomb",
        "local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 1024) .. i end",
    ),
    (
        "memory_bomb_in_pcall",
        "while true do pcall(function() local t = {} while true do table.insert(t, string.rep('y', 4096)) end end) end",
    ),
    (
        "string_doubling",
        "local s = 'x' while true do s = s .. s end",
    ),
    (
        "deep_recursion",
        "local function f(n) return f(n + 1) + 1 end f(1)",
    ),
    (
        "deep_recursion_at_memory_limit",
        "local hog = {} pcall(function() while true do hog[#hog + 1] = string.rep('z', 1024) .. #hog end end) \
         local function f(n) return f(n + 1) + 1 end f(1)",
    ),
    (
        "pcall_recursion_c_stack",
        "local function f() pcall(f) end f()",
    ),
    (
        "error_object_bomb",
        "local e = {} for i = 1, 1e7 do e[i] = i end error(e)",
    ),
    (
        "sleeping_loop_forever",
        "local t0 = now() while now() - t0 < 0.3 do sleep(0) end error('stopped by its own clock')",
    ),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("attack") => attack(args.get(2).expect("attack <name>")),
        Some("demo") => {
            let d = demo();
            println!("{d}");
            if d.contains("\"ok\":true") {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Some("reloads") => {
            let n = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(1000);
            let variant = args.get(3).map(String::as_str).unwrap_or("source");
            println!("{}", reloads(n, variant));
            ExitCode::SUCCESS
        }
        Some("report") | None => report(
            args.iter()
                .position(|a| a == "--json")
                .and_then(|i| args.get(i + 1)),
        ),
        Some(other) => {
            eprintln!("unknown command {other}");
            ExitCode::FAILURE
        }
    }
}

fn runtime() -> Runtime {
    Runtime::new(Config::default(), Std::Source(STD_SOURCE)).expect("runtime")
}

/// Run one attack. Exit 0 if the attack ended (stopped by an error, or unwound on its own) and the
/// VM still works afterwards; a host abort shows up to the parent as a signal instead.
fn attack(name: &str) -> ExitCode {
    let src = ATTACKS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .expect("unknown attack");
    let rt = runtime();
    let mem_before = rt.lua.used_memory();
    rt.spawn_chunk(name, src).expect("load attack");
    let t = Instant::now();
    rt.run(Duration::from_secs(10));
    let elapsed = t.elapsed();
    let outcome = rt
        .outcomes()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, o)| o);
    let _ = rt.lua.gc_collect();
    let _ = rt.lua.gc_collect();
    let mem_after = rt.lua.used_memory();
    rt.spawn_chunk(
        "sanity",
        "sleep(0.01) log('alive ' .. #std.map({1, 2, 3}, function(x) return x * 2 end))",
    )
    .unwrap();
    rt.run(Duration::from_secs(2));
    let sane = rt.log().iter().any(|l| l == "alive 3");
    let stopped = matches!(outcome, Some(Outcome::Failed(_)));
    let ended = outcome.is_some();
    let error = match &outcome {
        Some(Outcome::Failed(e)) => e.clone(),
        Some(Outcome::Finished) => "finished normally".into(),
        None => "still running after 10 s".into(),
    };
    println!(
        "{{\"attack\":{name:?},\"ended\":{ended},\"stopped\":{stopped},\"error\":{error:?},\"elapsed_ms\":{:.1},\"max_turn_ms\":{:.1},\
         \"mem_before_kib\":{},\"mem_after_kib\":{},\"vm_usable_after\":{sane},\"rss_kib\":{}}}",
        elapsed.as_secs_f64() * 1000.0,
        rt.max_turn().as_secs_f64() * 1000.0,
        mem_before / 1024,
        mem_after / 1024,
        rss_kib()
    );
    if ended && sane {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// The process model end to end: sleep, an HTTP call on a worker thread, waiting for an event
/// another task emits, sleeping inside pcall, scoped cleanup on error, and a timeout.
fn demo() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            std::thread::sleep(Duration::from_millis(30));
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            );
        }
    });
    let rt = runtime();
    let flow = format!(
        r#"
        local t0 = now()
        sleep(0.05)
        log(string.format("slept %.0f ms", (now() - t0) * 1000))
        log("http " .. http_get("http://127.0.0.1:{port}/"))
        log("event " .. wait("pinch"))
        pcall(function() sleep(0.01) log("slept inside pcall") end)
        local ok = pcall(std.with_scope, function(scope)
            scope:defer(function() log("cleanup ran") end)
            error("flow failed")
        end)
        log("scope error caught " .. tostring(not ok))
        local ok2, err = pcall(std.wait_for, "never", 0.05)
        log("timeout " .. tostring(not ok2 and string.find(tostring(err), "timed out") ~= nil))
        "#
    );
    rt.spawn_chunk("flow", &flow).unwrap();
    rt.spawn_chunk("gesture", "sleep(0.1) emit('pinch', 'right index')")
        .unwrap();
    let t = Instant::now();
    rt.run(Duration::from_secs(5));
    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
    let log = rt.log();
    let expect = [
        "http hello",
        "event right index",
        "slept inside pcall",
        "cleanup ran",
        "scope error caught true",
        "timeout true",
    ];
    let ok = expect.iter().all(|e| log.iter().any(|l| l == e))
        && log.first().is_some_and(|l| l.starts_with("slept "))
        && rt.outcomes().iter().all(|(_, o)| *o == Outcome::Finished)
        && rt.live_tasks() == 0;
    format!(
        "{{\"demo\":\"process model\",\"ok\":{ok},\"elapsed_ms\":{elapsed:.1},\"max_turn_ms\":{:.2},\"log\":{:?},\"outcomes\":{:?}}}",
        rt.max_turn().as_secs_f64() * 1000.0,
        log,
        rt.outcomes()
    )
}

const INIT: &str = r#"
local pinch = std.trigger.hold(std.trigger.pinch("right"), 0.2)
local follow = std.signal.spring(std.signal.offset(std.signal.landmark("right", 8), 0, -40))
local bindings = {}
for i = 1, 20 do
    bindings[i] = { trigger = std.trigger.any(pinch, std.trigger.pinch("left")), anchor = follow, id = i }
end
spawn(function() wait("never") end, "listener")
"#;

/// Reload `n` times: drop the VM, build a fresh one, load the standard library, run an init script.
fn reloads(n: usize, variant: &str) -> String {
    let large = padded_std(3000);
    let src: &str = if variant.starts_with("large") {
        &large
    } else {
        STD_SOURCE
    };
    let bytecode = compile_std(src).unwrap();
    let std_lines = src.lines().count();
    let mut times = Vec::with_capacity(n);
    let mut rss_at_100 = 0;
    let mut current: Option<Runtime> = None;
    for i in 0..n {
        let t = Instant::now();
        drop(current.take());
        let std = if variant.ends_with("bytecode") {
            Std::Bytecode(&bytecode)
        } else {
            Std::Source(src)
        };
        let rt = Runtime::new(Config::default(), std).expect("runtime");
        rt.exec("init", INIT).expect("init");
        rt.run(Duration::ZERO);
        current = Some(rt);
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        if i + 1 == 100.min(n) {
            rss_at_100 = rss_kib();
        }
    }
    let rss_end = rss_kib();
    let vm_mem = current.as_ref().map(|r| r.lua.used_memory()).unwrap_or(0);
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| sorted[((p * n as f64) as usize).min(n - 1)];
    format!(
        "{{\"reloads\":{n},\"variant\":{variant:?},\"std_lines\":{std_lines},\"ms_p50\":{:.2},\"ms_p95\":{:.2},\"ms_max\":{:.2},\
         \"rss_after_100_kib\":{rss_at_100},\"rss_end_kib\":{rss_end},\"rss_growth_kib\":{},\"vm_used_kib\":{}}}",
        q(0.5),
        q(0.95),
        sorted[n - 1],
        rss_end as i64 - rss_at_100 as i64,
        vm_mem / 1024
    )
}

fn run_child(args: &[&str], timeout: Duration) -> (Option<i32>, String, String) {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break Some(s);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut out = String::new();
    let mut err = String::new();
    let _ = child.stdout.take().unwrap().read_to_string(&mut out);
    let _ = child.stderr.take().unwrap().read_to_string(&mut err);
    let desc = match status {
        None => "timed out (hung)".to_string(),
        Some(s) => match (s.code(), std::os::unix::process::ExitStatusExt::signal(&s)) {
            (Some(c), _) => format!("exit {c}"),
            (None, Some(sig)) => format!("killed by signal {sig} (host aborted)"),
            _ => "unknown".into(),
        },
    };
    (
        status.and_then(|s| s.code()),
        out.trim().to_string(),
        format!("{desc} {}", err.trim()).trim().to_string(),
    )
}

fn report(json: Option<&String>) -> ExitCode {
    let mut lines = Vec::new();
    let mut all_survived = true;
    println!("== attacks (each in its own process; budget 50 ms per turn, VM limit 16 MiB)");
    for (name, _) in ATTACKS {
        let (code, out, status) = run_child(&["attack", name], Duration::from_secs(30));
        let survived = code == Some(0);
        all_survived &= survived;
        println!(
            "   {:<32} {:<8} {}",
            name,
            if survived { "survived" } else { "FAILED" },
            if survived {
                out.clone()
            } else {
                format!("{status} {out}")
            }
        );
        lines.push(if out.starts_with('{') {
            out
        } else {
            format!("{{\"attack\":{name:?},\"status\":{status:?}}}")
        });
    }
    println!("== process model demo");
    let (code, out, status) = run_child(&["demo"], Duration::from_secs(30));
    let demo_ok = code == Some(0);
    println!(
        "   {} {}",
        if demo_ok { "ok" } else { "FAILED" },
        if demo_ok {
            out.clone()
        } else {
            format!("{status} {out}")
        }
    );
    lines.push(out);
    println!("== 1000 reloads per variant (fresh VM + std + init each time)");
    let mut reload_ok = true;
    for variant in ["source", "bytecode", "large-source", "large-bytecode"] {
        let (_, out, status) = run_child(&["reloads", "1000", variant], Duration::from_secs(300));
        println!(
            "   {out} {}",
            if out.is_empty() {
                status
            } else {
                String::new()
            }
        );
        let field = |k: &str| -> f64 {
            out.split(&format!("\"{k}\":"))
                .nth(1)
                .and_then(|s| s.split([',', '}']).next()?.parse().ok())
                .unwrap_or(f64::NAN)
        };
        if variant == "source" {
            reload_ok = field("ms_p95") <= 50.0 && field("rss_growth_kib") <= 1024.0;
        }
        lines.push(out);
    }
    let go = all_survived && demo_ok && reload_ok;
    println!(
        "\n   pass bar: no case aborts the host {}  reload <=50 ms & memory flat {}  process model works {}  -> {}",
        if all_survived { "ok" } else { "FAIL" },
        if reload_ok { "ok" } else { "FAIL" },
        if demo_ok { "ok" } else { "FAIL" },
        if go { "GO" } else { "NO-GO" }
    );
    if let Some(path) = json {
        std::fs::write(path, format!("[\n{}\n]\n", lines.join(",\n"))).unwrap();
    }
    if go {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
