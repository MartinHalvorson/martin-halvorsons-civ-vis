//! Zero-dependency local HTTP server for the human-vs-AI browser GUI.
//! Endpoints: GET / (page), GET /state, GET /rules, POST /action, POST /new.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::ai::{AdvancedAi, Ai, BasicAi};
use crate::game::{Action, Game};
use crate::obs::{observation, observation_spectator};

const EMBEDDED_INDEX: &str = include_str!("../web/index.html");
const EMBEDDED_TERRAIN_ATLAS: &[u8] = include_bytes!("../web/assets/terrain-atlas.png");
const EMBEDDED_FEATURE_ATLAS: &[u8] = include_bytes!("../web/assets/feature-atlas.png");
const EMBEDDED_MOUNTAIN_ATLAS: &[u8] = include_bytes!("../web/assets/mountain-atlas.png");

#[derive(Clone)]
pub struct Params {
    pub num_players: usize,
    pub width: i32,
    pub height: i32,
    pub seed: u64,
    pub max_turns: u32,
    pub num_city_states: usize,
    /// All players AI-driven; the GUI just watches (auto-steps via /step).
    pub spectate: bool,
}

pub struct Session {
    pub params: Params,
    pub game: Game,
    ais: Vec<Box<dyn Ai + Send>>,
}

/// Server-side exhibition state: in spectate mode a background thread steps
/// the game at `pace_ms` per major turn and restarts 10s after a victory, so
/// games keep running with no browser attached.
pub struct Shared {
    pub session: Mutex<Session>,
    pub pace_ms: AtomicU64,
    pub paused: AtomicBool,
    pub restart_in: AtomicU64, // ms until auto-restart; u64::MAX = not pending
}

const RESTART_MS: u64 = 10_000;

impl Session {
    pub fn new(params: Params) -> Session {
        let game = Game::new_full(params.num_players, params.width, params.height,
                                  params.seed, params.max_turns,
                                  params.num_city_states, true);
        // Major civilizations use the coordinated strategic agent. A trained
        // value net remains the most specialized available agent; without one,
        // paired evaluation favors the advanced defaults over evolved weights.
        let champ = crate::evolve::load_champion("evolved");
        let net = crate::valuenet::ValueNet::load("evolved");
        let ais: Vec<Box<dyn Ai + Send>> = game.players.iter().map(|p| -> Box<dyn Ai + Send> {
            if p.is_minor || p.is_barbarian {
                return Box::new(BasicAi::new());
            }
            match (&champ, &net) {
                (Some(w), Some(n)) =>
                    Box::new(crate::neural::NeuralAi::new(w.clone(), n.clone())),
                (Some(_), None) => Box::new(AdvancedAi::new()),
                _ => Box::new(AdvancedAi::new()),
            }
        }).collect();
        Session { params, game, ais }
    }

    pub fn state(&self) -> Value {
        if self.params.spectate {
            let g = &self.game;
            // Observe from the current player's seat (fall back to the first
            // living major when a minor/barbarian is up).
            let pid = if g.players[g.current].is_minor {
                g.players.iter().find(|p| !p.is_minor && p.alive)
                    .map(|p| p.id).unwrap_or(0)
            } else {
                g.current
            };
            let mut o = observation_spectator(g, pid);
            o["spectate"] = json!(true);
            o["legal_actions"] = json!([]);
            return o;
        }
        let mut o = observation(&self.game, 0);
        o["legal_actions"] = serde_json::to_value(self.game.legal_actions(0)).unwrap();
        o
    }

    /// Spectator mode: play out exactly one player's turn with its AI.
    /// Returns the pid and successful actions so the observer UI can explain
    /// the AI's decisions instead of showing only their eventual outcomes.
    pub fn step(&mut self) -> (usize, Vec<Action>) {
        let g = &mut self.game;
        let pid = g.current;
        let log_start = g.log.len();
        if g.winner.is_some() {
            return (pid, vec![]);
        }
        self.ais[pid].take_turn(g, pid);
        if g.current == pid && g.winner.is_none() {
            let _ = g.apply(pid, &Action::EndTurn);
        }
        let actions = g.log[log_start..]
            .iter()
            .map(|(_, action)| action.clone())
            .collect();
        (pid, actions)
    }

    pub fn act(&mut self, v: &Value) -> Option<String> {
        let action: Action = match serde_json::from_value(v.clone()) {
            Ok(a) => a,
            Err(e) => return Some(format!("bad action: {e}")),
        };
        if let Err(e) = self.game.apply(0, &action) {
            return Some(e);
        }
        if matches!(action, Action::EndTurn) {
            let g = &mut self.game;
            let mut guard = 0;
            while g.winner.is_none() && g.current != 0 && g.players[0].alive
                && guard < 2 * g.players.len() {
                let pid = g.current;
                self.ais[pid].take_turn(g, pid);
                if g.current == pid && g.winner.is_none() {
                    let _ = g.apply(pid, &Action::EndTurn);
                }
                guard += 1;
            }
        }
        None
    }
}

fn index_html() -> Vec<u8> {
    for p in ["web/index.html"] {
        if let Ok(b) = std::fs::read(p) {
            return b;
        }
    }
    EMBEDDED_INDEX.as_bytes().to_vec()
}

fn terrain_atlas() -> Vec<u8> {
    std::fs::read("web/assets/terrain-atlas.png")
        .unwrap_or_else(|_| EMBEDDED_TERRAIN_ATLAS.to_vec())
}

fn feature_atlas() -> Vec<u8> {
    std::fs::read("web/assets/feature-atlas.png")
        .unwrap_or_else(|_| EMBEDDED_FEATURE_ATLAS.to_vec())
}

fn mountain_atlas() -> Vec<u8> {
    std::fs::read("web/assets/mountain-atlas.png")
        .unwrap_or_else(|_| EMBEDDED_MOUNTAIN_ATLAS.to_vec())
}

fn respond(stream: &mut TcpStream, code: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len());
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn respond_json(stream: &mut TcpStream, v: &Value) {
    respond(stream, "200 OK", "application/json", v.to_string().as_bytes());
}

fn auto_step_loop(sh: Arc<Shared>) {
    let mut over_since: Option<Instant> = None;
    loop {
        let pace = sh.pace_ms.load(Ordering::Relaxed).clamp(20, 60_000);
        if sh.paused.load(Ordering::Relaxed) {
            over_since = None; // pausing resets the restart countdown
            std::thread::sleep(Duration::from_millis(150));
            continue;
        }
        let mut delay = pace;
        {
            let mut s = sh.session.lock().unwrap();
            if !s.params.spectate {
                drop(s);
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
            if s.game.winner.is_some() {
                let t0 = *over_since.get_or_insert_with(Instant::now);
                let left = RESTART_MS.saturating_sub(t0.elapsed().as_millis() as u64);
                sh.restart_in.store(left, Ordering::Relaxed);
                if left == 0 {
                    let mut p = s.params.clone();
                    p.seed = p.seed.wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    *s = Session::new(p);
                    over_since = None;
                    sh.restart_in.store(u64::MAX, Ordering::Relaxed);
                }
                delay = 200;
            } else {
                over_since = None;
                sh.restart_in.store(u64::MAX, Ordering::Relaxed);
                let (pid, _) = s.step();
                let p = &s.game.players[pid];
                if p.is_minor || p.is_barbarian {
                    delay = (pace / 4).max(30); // quick beat for minors
                }
            }
        }
        std::thread::sleep(Duration::from_millis(delay));
    }
}

/// Attach exhibition metadata (restart countdown, pace, paused) to a state.
fn decorate(o: &mut Value, sh: &Shared) {
    let r = sh.restart_in.load(Ordering::Relaxed);
    if r != u64::MAX {
        o["restart_in"] = json!(r.div_ceil(1000));
    }
    o["pace"] = json!(sh.pace_ms.load(Ordering::Relaxed));
    o["paused"] = json!(sh.paused.load(Ordering::Relaxed));
}

fn handle(stream: &mut TcpStream, sh: &Shared) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();
    let mut content_len = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h == "\r\n" || h == "\n" || h.is_empty() {
            break;
        }
        let hl = h.to_ascii_lowercase();
        if let Some(v) = hl.strip_prefix("content-length:") {
            content_len = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        let _ = reader.read_exact(&mut body);
    }
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    match (method.as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => {
            respond(stream, "200 OK", "text/html; charset=utf-8", &index_html());
        }
        ("GET", "/assets/terrain-atlas.png") => {
            respond(stream, "200 OK", "image/png", &terrain_atlas());
        }
        ("GET", "/assets/feature-atlas.png") => {
            respond(stream, "200 OK", "image/png", &feature_atlas());
        }
        ("GET", "/assets/mountain-atlas.png") => {
            respond(stream, "200 OK", "image/png", &mountain_atlas());
        }
        ("GET", "/state") => {
            let mut o = sh.session.lock().unwrap().state();
            decorate(&mut o, sh);
            respond_json(stream, &o);
        }
        ("POST", "/pace") => {
            if let Some(v) = parsed["ms"].as_u64() {
                sh.pace_ms.store(v.clamp(20, 60_000), Ordering::Relaxed);
            }
            if let Some(v) = parsed["paused"].as_bool() {
                sh.paused.store(v, Ordering::Relaxed);
            }
            let mut o = sh.session.lock().unwrap().state();
            decorate(&mut o, sh);
            respond_json(stream, &o);
        }
        ("GET", "/rules") => {
            let session = sh.session.lock().unwrap();
            let r = &session.game.rules;
            respond_json(stream, &json!({
                "techs": r.techs, "civics": r.civics,
                "terrains": r.terrains, "features": r.features,
                "resources": r.resources, "improvements": r.improvements,
                "governments": r.governments, "units": r.units,
                "buildings": r.buildings, "districts": r.districts,
                "projects": r.projects,
                "policies": r.policies, "beliefs": r.beliefs, "civs": r.civs,
            }));
        }
        ("POST", "/action") => {
            let mut session = sh.session.lock().unwrap();
            let err = session.act(&parsed["action"]);
            let mut out = session.state();
            out["error"] = match err {
                Some(e) => Value::String(e),
                None => Value::Null,
            };
            respond_json(stream, &out);
        }
        ("POST", "/step") => {
            let mut session = sh.session.lock().unwrap();
            let mut out;
            if session.params.spectate {
                let (pid, actions) = session.step();
                out = session.state();
                out["stepped"] = json!(pid);
                out["actions_taken"] = serde_json::to_value(actions).unwrap();
            } else {
                out = session.state();
                out["error"] = json!("not in spectate mode");
            }
            drop(session);
            decorate(&mut out, sh);
            respond_json(stream, &out);
        }
        ("POST", "/new") => {
            let mut session = sh.session.lock().unwrap();
            let mut p = session.params.clone();
            if let Some(v) = parsed["num_players"].as_u64() {
                p.num_players = v as usize;
            }
            if let Some(v) = parsed["seed"].as_u64() {
                p.seed = v;
            }
            if let Some(v) = parsed["width"].as_i64() {
                p.width = v as i32;
            }
            if let Some(v) = parsed["height"].as_i64() {
                p.height = v as i32;
            }
            if let Some(v) = parsed["num_city_states"].as_u64() {
                p.num_city_states = v as usize;
            }
            if let Some(v) = parsed["spectate"].as_bool() {
                p.spectate = v;
            }
            *session = Session::new(p);
            let mut o = session.state();
            drop(session);
            decorate(&mut o, sh);
            respond_json(stream, &o);
        }
        _ => respond(stream, "404 Not Found", "application/json",
                     b"{\"error\":\"not found\"}"),
    }
}

pub fn serve(port: u16, open_browser: bool, params: Params) {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .unwrap_or_else(|e| panic!("cannot bind port {port}: {e}"));
    let actual = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{actual}/");
    println!("Martin Halvorson's Civilization VIS — playing at {url}");
    if params.spectate {
        println!("Spectator mode: all {} players are AI-driven. Ctrl+C to quit.",
                 params.num_players);
    } else {
        println!("You are player 0. Ctrl+C to quit.");
    }
    let shared = Arc::new(Shared {
        session: Mutex::new(Session::new(params)),
        pace_ms: AtomicU64::new(100), // lightning by default
        paused: AtomicBool::new(false),
        restart_in: AtomicU64::new(u64::MAX),
    });
    let stepper = shared.clone();
    std::thread::spawn(move || auto_step_loop(stepper));
    if open_browser {
        open_url(&url);
    }
    for stream in listener.incoming() {
        if let Ok(mut s) = stream {
            handle(&mut s, &shared);
        }
    }
}

fn open_url(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
