//! Spike 4: does mlua with Luau support the handOs process model?
//!
//! A process is a Luau function run as an mlua async task. A small single-threaded executor
//! drives the tasks (no async runtime): `sleep` uses a timer heap, `wait` parks on a named
//! event, and `http_get` runs on at most two worker threads. Each poll of a task is one
//! "turn"; an interrupt aborts any turn that runs past the hard budget.

use std::cell::{Cell, RefCell};
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use mlua::chunk::{ChunkMode, Compiler};
use mlua::{Function, Lua, LuaOptions, StdLib, Value, VmState};

pub const STD_SOURCE: &str = include_str!("std.luau");

#[derive(Clone, Debug)]
pub struct Config {
    /// Hard budget for one turn of one task.
    pub budget: Duration,
    /// Memory limit for the whole VM, in bytes.
    pub memory_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            budget: Duration::from_millis(50),
            memory_limit: 16 << 20,
        }
    }
}

/// How the standard library is loaded on each reload.
pub enum Std<'a> {
    Source(&'a str),
    Bytecode(&'a [u8]),
}

pub fn compile_std(src: &str) -> mlua::Result<Vec<u8>> {
    Compiler::new().set_optimization_level(2).compile(src)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Finished,
    Failed(String),
}

// ------------------------------------------------------------------ executor

struct Ready {
    queue: Mutex<VecDeque<usize>>,
    signal: Condvar,
}

struct TaskWaker {
    id: usize,
    ready: Arc<Ready>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.ready.queue.lock().unwrap().push_back(self.id);
        self.ready.signal.notify_one();
    }
}

struct Timer {
    at: Instant,
    waker: Waker,
}

impl PartialEq for Timer {
    fn eq(&self, o: &Self) -> bool {
        self.at == o.at
    }
}
impl Eq for Timer {}
impl PartialOrd for Timer {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Timer {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.at.cmp(&self.at) // min-heap on deadline
    }
}

struct Task {
    name: String,
    fut: Pin<Box<dyn Future<Output = mlua::Result<()>>>>,
}

#[derive(Default)]
struct EventSlot {
    value: RefCell<Option<String>>,
    waker: RefCell<Option<Waker>>,
}

type Job = Box<dyn FnOnce() + Send>;

struct Workers {
    tx: mpsc::Sender<Job>,
    rx: Arc<Mutex<mpsc::Receiver<Job>>>,
    started: Cell<usize>,
}

impl Workers {
    const MAX: usize = 2;

    fn new() -> Workers {
        let (tx, rx) = mpsc::channel();
        Workers {
            tx,
            rx: Arc::new(Mutex::new(rx)),
            started: Cell::new(0),
        }
    }

    fn submit(&self, job: Job) {
        if self.started.get() < Self::MAX {
            self.started.set(self.started.get() + 1);
            let rx = self.rx.clone();
            std::thread::spawn(move || {
                loop {
                    let job = { rx.lock().unwrap().recv() };
                    match job {
                        Ok(job) => job(),
                        Err(_) => return,
                    }
                }
            });
        }
        let _ = self.tx.send(job);
    }
}

struct Sched {
    ready: Arc<Ready>,
    timers: RefCell<BinaryHeap<Timer>>,
    events: RefCell<HashMap<String, Vec<Rc<EventSlot>>>>,
    tasks: RefCell<Vec<Option<Task>>>,
    live: Cell<usize>,
    turn_start: Rc<Cell<Instant>>,
    workers: Workers,
    log: RefCell<Vec<String>>,
    outcomes: RefCell<Vec<(String, Outcome)>>,
    max_turn: Cell<Duration>,
    /// The task being polled; its slot is empty during the poll but must not be reused.
    running: Cell<Option<usize>>,
}

impl Sched {
    fn spawn(&self, name: String, fut: Pin<Box<dyn Future<Output = mlua::Result<()>>>>) -> usize {
        let mut tasks = self.tasks.borrow_mut();
        let running = self.running.get();
        let free = tasks
            .iter()
            .enumerate()
            .position(|(k, t)| t.is_none() && Some(k) != running);
        let id = free.unwrap_or_else(|| {
            tasks.push(None);
            tasks.len() - 1
        });
        tasks[id] = Some(Task { name, fut });
        self.live.set(self.live.get() + 1);
        self.ready.queue.lock().unwrap().push_back(id);
        id
    }

    fn emit(&self, event: &str, value: String) -> usize {
        let slots = self.events.borrow_mut().remove(event).unwrap_or_default();
        for s in &slots {
            *s.value.borrow_mut() = Some(value.clone());
            if let Some(w) = s.waker.borrow_mut().take() {
                w.wake();
            }
        }
        slots.len()
    }
}

struct Sleep {
    at: Instant,
    sched: Rc<Sched>,
}

impl Future for Sleep {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if Instant::now() >= self.at {
            return Poll::Ready(());
        }
        self.sched.timers.borrow_mut().push(Timer {
            at: self.at,
            waker: cx.waker().clone(),
        });
        Poll::Pending
    }
}

struct WaitEvent {
    name: String,
    slot: Option<Rc<EventSlot>>,
    sched: Rc<Sched>,
}

impl Future for WaitEvent {
    type Output = String;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<String> {
        if let Some(slot) = &self.slot
            && let Some(v) = slot.value.borrow_mut().take()
        {
            return Poll::Ready(v);
        }
        let this = &mut *self;
        let slot = this.slot.get_or_insert_with(|| {
            let slot = Rc::new(EventSlot::default());
            this.sched
                .events
                .borrow_mut()
                .entry(this.name.clone())
                .or_default()
                .push(slot.clone());
            slot
        });
        *slot.waker.borrow_mut() = Some(cx.waker().clone());
        Poll::Pending
    }
}

type Shared<T> = Arc<Mutex<(Option<T>, Option<Waker>)>>;

struct Offload<T> {
    shared: Shared<T>,
}

impl<T> Future for Offload<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut g = self.shared.lock().unwrap();
        match g.0.take() {
            Some(v) => Poll::Ready(v),
            None => {
                g.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

fn offload<T: Send + 'static>(sched: &Sched, f: impl FnOnce() -> T + Send + 'static) -> Offload<T> {
    let shared: Shared<T> = Arc::new(Mutex::new((None, None)));
    let s2 = shared.clone();
    sched.workers.submit(Box::new(move || {
        let v = f();
        let mut g = s2.lock().unwrap();
        g.0 = Some(v);
        if let Some(w) = g.1.take() {
            w.wake();
        }
    }));
    Offload { shared }
}

// ------------------------------------------------------------------ runtime

pub struct Runtime {
    pub lua: Lua,
    sched: Rc<Sched>,
    cfg: Config,
}

impl Runtime {
    /// A fresh VM: host API, standard library, sandbox, limits. Every reload builds a new one.
    pub fn new(cfg: Config, std: Std) -> mlua::Result<Runtime> {
        let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())?;
        let sched = Rc::new(Sched {
            ready: Arc::new(Ready {
                queue: Mutex::new(VecDeque::new()),
                signal: Condvar::new(),
            }),
            timers: RefCell::new(BinaryHeap::new()),
            events: RefCell::new(HashMap::new()),
            tasks: RefCell::new(Vec::new()),
            live: Cell::new(0),
            turn_start: Rc::new(Cell::new(Instant::now())),
            workers: Workers::new(),
            log: RefCell::new(Vec::new()),
            outcomes: RefCell::new(Vec::new()),
            max_turn: Cell::new(Duration::ZERO),
            running: Cell::new(None),
        });

        let budget = cfg.budget;
        let turn_start = sched.turn_start.clone();
        lua.set_interrupt(move |_| {
            if turn_start.get().elapsed() > budget {
                Err(mlua::Error::runtime(format!(
                    "script exceeded its {} ms budget",
                    budget.as_millis()
                )))
            } else {
                Ok(VmState::Continue)
            }
        });

        let g = lua.globals();
        let s = sched.clone();
        g.set(
            "sleep",
            lua.create_async_function(move |_, secs: f64| {
                let at = Instant::now() + Duration::from_secs_f64(secs.max(0.0));
                let fut = Sleep {
                    at,
                    sched: s.clone(),
                };
                async move {
                    fut.await;
                    Ok(())
                }
            })?,
        )?;
        let s = sched.clone();
        g.set(
            "wait",
            lua.create_async_function(move |_, name: String| {
                let fut = WaitEvent {
                    name,
                    slot: None,
                    sched: s.clone(),
                };
                async move { Ok(fut.await) }
            })?,
        )?;
        let s = sched.clone();
        g.set(
            "http_get",
            lua.create_async_function(move |_, url: String| {
                let fut = offload(&s, move || -> Result<String, String> {
                    let mut resp = ureq::get(&url).call().map_err(|e| e.to_string())?;
                    resp.body_mut().read_to_string().map_err(|e| e.to_string())
                });
                async move { fut.await.map_err(mlua::Error::runtime) }
            })?,
        )?;
        let s = sched.clone();
        g.set(
            "emit",
            lua.create_function(move |_, (name, value): (String, Option<String>)| {
                Ok(s.emit(&name, value.unwrap_or_default()))
            })?,
        )?;
        let s = sched.clone();
        g.set(
            "spawn",
            lua.create_function(move |_, (f, name): (Function, Option<String>)| {
                let fut = f.call_async::<()>(());
                Ok(s.spawn(name.unwrap_or_else(|| "task".into()), Box::pin(fut)))
            })?,
        )?;
        let s = sched.clone();
        g.set(
            "log",
            lua.create_function(move |_, msg: String| {
                s.log.borrow_mut().push(msg);
                Ok(())
            })?,
        )?;
        let t0 = Instant::now();
        g.set(
            "now",
            lua.create_function(move |_, ()| Ok(t0.elapsed().as_secs_f64()))?,
        )?;

        sched.turn_start.set(Instant::now());
        let std_table: Value = match std {
            Std::Source(src) => lua
                .load(src)
                .set_name("=std")
                .set_mode(ChunkMode::Text)
                .eval()?,
            Std::Bytecode(bc) => lua
                .load(bc)
                .set_name("=std")
                .set_mode(ChunkMode::Binary)
                .eval()?,
        };
        g.set("std", std_table)?;
        lua.sandbox(true)?;
        lua.set_memory_limit(cfg.memory_limit)?;
        Ok(Runtime { lua, sched, cfg })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Run a chunk of user code (its top level is one turn).
    pub fn exec(&self, name: &str, src: &str) -> mlua::Result<()> {
        self.sched.turn_start.set(Instant::now());
        self.lua.load(src).set_name(format!("={name}")).exec()
    }

    /// Start `src` (a chunk that is a function body) as a process.
    pub fn spawn_chunk(&self, name: &str, src: &str) -> mlua::Result<usize> {
        let f = self
            .lua
            .load(src)
            .set_name(format!("={name}"))
            .into_function()?;
        Ok(self
            .sched
            .spawn(name.into(), Box::pin(f.call_async::<()>(()))))
    }

    pub fn emit(&self, event: &str, value: &str) -> usize {
        self.sched.emit(event, value.into())
    }

    pub fn log(&self) -> Vec<String> {
        self.sched.log.borrow().clone()
    }

    pub fn outcomes(&self) -> Vec<(String, Outcome)> {
        self.sched.outcomes.borrow().clone()
    }

    pub fn live_tasks(&self) -> usize {
        self.sched.live.get()
    }

    pub fn max_turn(&self) -> Duration {
        self.sched.max_turn.get()
    }

    /// Drive tasks until none is left or `limit` passes.
    pub fn run(&self, limit: Duration) {
        let end = Instant::now() + limit;
        loop {
            let now = Instant::now();
            loop {
                let due = matches!(self.sched.timers.borrow().peek(), Some(t) if t.at <= now);
                if !due {
                    break;
                }
                let t = self.sched.timers.borrow_mut().pop().unwrap();
                t.waker.wake();
            }
            let next = self.sched.ready.queue.lock().unwrap().pop_front();
            if let Some(id) = next {
                self.poll_task(id);
                continue;
            }
            if self.sched.live.get() == 0 || now >= end {
                return;
            }
            let until_timer = self
                .sched
                .timers
                .borrow()
                .peek()
                .map(|t| t.at.saturating_duration_since(now));
            let wait = until_timer.unwrap_or(Duration::MAX).min(end - now);
            let q = self.sched.ready.queue.lock().unwrap();
            if q.is_empty() {
                let _ = self.sched.ready.signal.wait_timeout(q, wait).unwrap();
            }
        }
    }

    fn poll_task(&self, id: usize) {
        let Some(mut task) = self
            .sched
            .tasks
            .borrow_mut()
            .get_mut(id)
            .and_then(Option::take)
        else {
            return; // already finished; a stale wake-up
        };
        let waker = Waker::from(Arc::new(TaskWaker {
            id,
            ready: self.sched.ready.clone(),
        }));
        let start = Instant::now();
        self.sched.turn_start.set(start);
        self.sched.running.set(Some(id));
        let res = task.fut.as_mut().poll(&mut Context::from_waker(&waker));
        self.sched.running.set(None);
        let turn = start.elapsed();
        if turn > self.sched.max_turn.get() {
            self.sched.max_turn.set(turn);
        }
        match res {
            Poll::Pending => self.sched.tasks.borrow_mut()[id] = Some(task),
            Poll::Ready(r) => {
                self.sched.live.set(self.sched.live.get() - 1);
                let outcome = match r {
                    Ok(()) => Outcome::Finished,
                    Err(e) => Outcome::Failed(first_line(&e.to_string())),
                };
                self.sched.outcomes.borrow_mut().push((task.name, outcome));
            }
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

/// A standard library of about `lines` lines, for measuring how reload time scales with its size.
pub fn padded_std(lines: usize) -> String {
    let mut s = String::from(
        STD_SOURCE
            .trim_end()
            .strip_suffix("return std")
            .expect("std ends with `return std`"),
    );
    let mut n = 0;
    let mut k = 0;
    while n < lines {
        s.push_str(&format!(
            "function std.generated_{k}(items: {{number}}, scale: number): {{number}}\n\
             \tlocal out = table.create(#items)\n\
             \tfor i, v in items do\n\
             \t\tif v > {k} then out[i] = v * scale else out[i] = v / (scale + 1) end\n\
             \tend\n\
             \treturn out\n\
             end\n\
             std.table_{k} = {{ name = \"generated_{k}\", weight = {k}, tags = {{\"a\", \"b\"}} }}\n"
        ));
        n += 8;
        k += 1;
    }
    s.push_str("return std\n");
    s
}

pub fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or(0)
}
