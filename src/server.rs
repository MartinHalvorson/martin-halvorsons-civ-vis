//! Zero-dependency local HTTP server for the human-vs-AI browser GUI.
//! Endpoints: GET / (page), GET /state, GET /rules, POST /action, POST /new.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use serde_json::{json, Value};

use crate::ai::{Ai, BasicAi};
use crate::game::{Action, Game};
use crate::obs::{observation, observation_spectator};
use crate::setup::{MapSize, CIV6_MAP_SIZES};

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
    ais: Vec<Box<dyn Ai>>,
}

impl Session {
    pub fn new(params: Params) -> Session {
        let game = Game::new_full(params.num_players, params.width, params.height,
                                  params.seed, params.max_turns,
                                  params.num_city_states, true);
        // majors use the strongest available AI: value-net NeuralAi when a
        // trained net exists, else evolved champion weights, else defaults
        let champ = crate::evolve::load_champion("evolved");
        let net = crate::valuenet::ValueNet::load("evolved");
        let ais: Vec<Box<dyn Ai>> = game.players.iter().map(|p| -> Box<dyn Ai> {
            if p.is_minor || p.is_barbarian {
                return Box::new(BasicAi::new());
            }
            match (&champ, &net) {
                (Some(w), Some(n)) =>
                    Box::new(crate::neural::NeuralAi::new(w.clone(), n.clone())),
                (Some(w), None) => Box::new(BasicAi::with_weights(w.clone())),
                _ => Box::new(BasicAi::new()),
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

fn new_game_params(current: &Params, request: &Value) -> Params {
    let mut p = current.clone();
    if let Some(v) = request["num_players"].as_u64() {
        p.num_players = v as usize;
        let size = MapSize::for_players(p.num_players);
        p.width = size.width;
        p.height = size.height;
        p.num_city_states = size.default_city_states;
    }
    if let Some(v) = request["seed"].as_u64() {
        p.seed = v;
    }
    // Advanced clients can still deliberately override individual stock
    // settings by sending them alongside num_players.
    if let Some(v) = request["width"].as_i64() {
        p.width = v as i32;
    }
    if let Some(v) = request["height"].as_i64() {
        p.height = v as i32;
    }
    if let Some(v) = request["num_city_states"].as_u64() {
        p.num_city_states = v as usize;
    }
    if let Some(v) = request["spectate"].as_bool() {
        p.spectate = v;
    }
    p
}

fn handle(stream: &mut TcpStream, session: &mut Session) {
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
        ("GET", "/state") => respond_json(stream, &session.state()),
        ("GET", "/rules") => {
            let r = &session.game.rules;
            respond_json(stream, &json!({
                "techs": r.techs, "civics": r.civics,
                "terrains": r.terrains, "features": r.features,
                "resources": r.resources, "improvements": r.improvements,
                "governments": r.governments, "units": r.units,
                "buildings": r.buildings, "districts": r.districts,
                "projects": r.projects,
                "policies": r.policies, "beliefs": r.beliefs, "civs": r.civs,
                "map_sizes": CIV6_MAP_SIZES,
            }));
        }
        ("POST", "/action") => {
            let err = session.act(&parsed["action"]);
            let mut out = session.state();
            out["error"] = match err {
                Some(e) => Value::String(e),
                None => Value::Null,
            };
            respond_json(stream, &out);
        }
        ("POST", "/step") => {
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
            respond_json(stream, &out);
        }
        ("POST", "/new") => {
            let p = new_game_params(&session.params, &parsed);
            *session = Session::new(p);
            respond_json(stream, &session.state());
        }
        _ => respond(stream, "404 Not Found", "application/json",
                     b"{\"error\":\"not found\"}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{new_game_params, Params};
    use serde_json::json;

    fn current() -> Params {
        Params {
            num_players: 2,
            width: 20,
            height: 14,
            seed: 1,
            max_turns: 500,
            num_city_states: 1,
            spectate: false,
        }
    }

    #[test]
    fn new_game_player_count_applies_the_whole_civ6_size_profile() {
        let tiny = new_game_params(&current(), &json!({"num_players": 4}));
        assert_eq!((tiny.width, tiny.height, tiny.num_city_states), (60, 38, 6));

        let small = new_game_params(&tiny, &json!({"num_players": 6}));
        assert_eq!((small.width, small.height, small.num_city_states), (74, 46, 9));
    }

    #[test]
    fn explicit_advanced_overrides_win_over_the_profile() {
        let p = new_game_params(&current(), &json!({
            "num_players": 6,
            "width": 80,
            "height": 50,
            "num_city_states": 2
        }));
        assert_eq!((p.width, p.height, p.num_city_states), (80, 50, 2));
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
    let mut session = Session::new(params);
    if open_browser {
        open_url(&url);
    }
    for stream in listener.incoming() {
        if let Ok(mut s) = stream {
            handle(&mut s, &mut session);
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
