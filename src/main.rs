mod json;
mod linux;
use json::{Json, parse, quote};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
const DEFAULT: &str = r#"{"active":"Основной","profiles":[{"name":"Основной","bindings":{}}]}"#;
struct State {
    config: Json,
    config_path: PathBuf,
    running: bool,
    auto_connect: bool,
    learning: bool,
    device: String,
    error: String,
    last_key: u16,
    sequence: u64,
}
enum Command {
    Start(String),
    Auto,
    Stop,
}
fn validate(c: &Json) -> Result<(), String> {
    let profiles = c.get("profiles").arr();
    if profiles.is_empty() || profiles.len() > 32 {
        return Err("Нужно от 1 до 32 профилей".into());
    }
    let mut names = HashSet::new();
    for p in profiles {
        let name = p.get("name").str();
        if name.is_empty() || !names.insert(name) {
            return Err("Имена профилей должны быть уникальны".into());
        }
        if !matches!(p.get("layout"), Json::Null) {
            let layout = p.get("layout").arr();
            let mut seen = HashSet::new();
            if layout.len() != 19
                || layout
                    .iter()
                    .any(|n| !matches!(n, Json::Num(1..=767)) || !seen.insert(n.num()))
            {
                return Err("Раскладка должна содержать 19 разных кодов от 1 до 767".into());
            }
        }
        if !matches!(p.get("wheel"), Json::Null) {
            let wheel = p.get("wheel").arr();
            let mut seen = HashSet::new();
            if wheel.len() != 3
                || wheel.iter().any(|n| match n {
                    Json::Null => false,
                    Json::Num(1..=767) => {
                        !seen.insert(n.num()) || p.get("layout").arr().iter().any(|k| k == n)
                    }
                    _ => true,
                })
            {
                return Err("Колёсико: три кода (1–767) или null, без повторов".into());
            }
        }
        let Json::Obj(bindings) = p.get("bindings") else {
            return Err("Нет назначений".into());
        };
        if bindings.len() > 767 {
            return Err("Слишком много назначений".into());
        }
        for (k, b) in bindings {
            let key: u16 = k.parse().map_err(|_| "Код клавиши должен быть числом")?;
            if key == 0 || key > 767 {
                return Err("Неверный код клавиши".into());
            }
            if !matches!(b.get("label"), Json::Null | Json::Str(_)) {
                return Err("Название кнопки должно быть строкой".into());
            }
            match b.get("kind").str() {
                "disabled" => {}
                "chord" => check_keys(b.get("keys"))?,
                "macro" => {
                    let steps = b.get("steps").arr();
                    if steps.is_empty() || steps.len() > 64 {
                        return Err("Макрос: 1–64 шага".into());
                    }
                    let mut delay = 0;
                    for s in steps {
                        check_keys(s.get("keys"))?;
                        if !matches!(s.get("delay"), Json::Num(_)) {
                            return Err("Пауза должна быть целым числом".into());
                        }
                        let n = s.get("delay").num();
                        if !(0..=2000).contains(&n) {
                            return Err("Пауза: 0–2000 мс".into());
                        }
                        delay += n;
                    }
                    if delay > 5000 {
                        return Err("Суммарная пауза не больше 5 секунд".into());
                    }
                }
                _ => return Err("Неизвестное действие".into()),
            }
        }
    }
    if !names.contains(c.get("active").str()) {
        return Err("Активный профиль не существует".into());
    }
    Ok(())
}
fn check_keys(k: &Json) -> Result<(), String> {
    let a = k.arr();
    if a.is_empty() || a.len() > 8 || a.iter().any(|n| !matches!(n, Json::Num(1..=767))) {
        Err("Сочетание: 1–8 Linux keycodes (1–767)".into())
    } else {
        let mut seen = HashSet::new();
        if a.iter().any(|n| !seen.insert(n.num())) {
            Err("Повтор клавиши в сочетании".into())
        } else {
            Ok(())
        }
    }
}
fn keys(v: &Json) -> Vec<u16> {
    v.arr().iter().map(|n| n.num() as u16).collect()
}
fn worker(state: Arc<Mutex<State>>, rx: mpsc::Receiver<Command>) {
    let mut input: Option<linux::Input> = None;
    let mut output: Option<linux::Output> = None;
    let mut held: HashMap<u16, Vec<u16>> = HashMap::new();
    let mut counts: HashMap<u16, usize> = HashMap::new();
    let mut next_scan = Instant::now();
    loop {
        let command = rx.try_recv().ok().or_else(|| {
            if input.is_none() && Instant::now() >= next_scan && state.lock().unwrap().auto_connect
            {
                next_scan = Instant::now() + Duration::from_secs(2);
                return linux::preferred_device(&linux::devices()).map(Command::Start);
            }
            None
        });
        if let Some(command) = command {
            let command = match command {
                Command::Auto => {
                    Command::Start(linux::preferred_device(&linux::devices()).unwrap_or_default())
                }
                command => command,
            };
            if let Some(o) = output.as_mut() {
                for k in counts.keys() {
                    let _ = o.key(*k, 0);
                }
            }
            held.clear();
            counts.clear();
            input = None;
            output = None;
            let mut s = state.lock().unwrap();
            s.running = false;
            s.learning = false;
            s.device.clear();
            s.error.clear();
            s.auto_connect = !matches!(command, Command::Stop);
            next_scan = Instant::now() + Duration::from_secs(2);
            if let Command::Start(path) = command {
                if path.is_empty() {
                    continue;
                }
                let result = (|| {
                    if !linux::devices().iter().any(|(p, _)| p == &path) {
                        return Err(std::io::Error::other(
                            "Разрешена только клавиатура 30fa:2350",
                        ));
                    }
                    if let Some(error) = linux::access_error(&path) {
                        return Err(std::io::Error::other(error));
                    }
                    let mut i = linux::Input::open(&path)?;
                    let o = linux::Output::new().map_err(|e| std::io::Error::other(format!("Не удалось создать виртуальную клавиатуру через /dev/uinput: {e}. Проверьте загрузку модуля uinput и права по README.")))?;
                    i.grab(true)?;
                    Ok((i, o))
                })();
                match result {
                    Ok((i, o)) => {
                        input = Some(i);
                        output = Some(o);
                        s.running = true;
                        s.device = path
                    }
                    Err(e) => s.error = e.to_string(),
                }
            }
        }
        let result = (|| -> std::io::Result<()> {
            let (Some(i), Some(o)) = (input.as_mut(), output.as_mut()) else {
                return Ok(());
            };
            for _ in 0..64 {
                let Some(e) = i.read()? else { break };
                if e.kind == 0 && e.code == 3 {
                    return Err(std::io::Error::other(
                        "Потеря событий: перезапустите перехват",
                    ));
                }
                if e.kind != 1 {
                    continue;
                }
                if e.value == 1 {
                    let binding = {
                        let mut s = state.lock().unwrap();
                        s.last_key = e.code;
                        s.sequence += 1;
                        if s.learning {
                            s.learning = false;
                            held.insert(e.code, vec![]);
                            continue;
                        }
                        let p = s
                            .config
                            .get("profiles")
                            .arr()
                            .iter()
                            .find(|p| p.get("name").str() == s.config.get("active").str());
                        p.map(|p| p.get("bindings").get(&e.code.to_string()).clone())
                            .unwrap_or(Json::Null)
                    };
                    if held.contains_key(&e.code) {
                        continue;
                    }
                    if binding.get("kind").str() == "macro" {
                        held.insert(e.code, vec![]);
                        for step in binding.get("steps").arr() {
                            let ks = keys(step.get("keys"));
                            for k in &ks {
                                if !counts.contains_key(k) {
                                    o.key(*k, 1)?
                                }
                            }
                            for k in ks.iter().rev() {
                                if !counts.contains_key(k) {
                                    o.key(*k, 0)?
                                }
                            }
                            thread::sleep(Duration::from_millis(step.get("delay").num() as u64));
                        }
                        continue;
                    }
                    let target = match binding.get("kind").str() {
                        "disabled" => vec![],
                        "chord" => keys(binding.get("keys")),
                        _ => vec![e.code],
                    };
                    for k in &target {
                        let n = counts.entry(*k).or_default();
                        if *n == 0 {
                            o.key(*k, 1)?
                        }
                        *n += 1;
                    }
                    held.insert(e.code, target);
                } else if e.value == 0 {
                    if let Some(ks) = held.remove(&e.code) {
                        for k in ks.iter().rev() {
                            if let Some(n) = counts.get_mut(k) {
                                *n -= 1;
                                if *n == 0 {
                                    o.key(*k, 0)?;
                                    counts.remove(k);
                                }
                            }
                        }
                    }
                } else if e.value == 2
                    && let Some(ks) = held.get(&e.code)
                    && let Some(k) = ks.last()
                {
                    o.key(*k, 2)?
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            if let Some(o) = output.as_mut() {
                for k in counts.keys() {
                    let _ = o.key(*k, 0);
                }
            }
            input = None;
            output = None;
            held.clear();
            counts.clear();
            let mut s = state.lock().unwrap();
            s.running = false;
            s.learning = false;
            s.error = e.to_string();
        }
        thread::sleep(Duration::from_millis(5));
    }
}
fn response(stream: &mut impl Write, status: &str, ctype: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'\r\n\r\n{body}",
        body.len()
    );
}
fn serve(mut stream: impl Read + Write, state: Arc<Mutex<State>>, tx: mpsc::Sender<Command>) {
    let mut data = Vec::new();
    let end = loop {
        let mut b = [0; 4096];
        let Ok(n) = stream.read(&mut b) else { return };
        if n == 0 {
            return;
        }
        data.extend_from_slice(&b[..n]);
        if data.len() > 1_100_000 {
            return;
        }
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let header = String::from_utf8_lossy(&data[..end]).into_owned();
    let mut lines = header.lines();
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("");
    let path = first.next().unwrap_or("");
    let headers: HashMap<_, _> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    if headers.get("host").map(String::as_str) != Some("127.0.0.1:8765")
        && headers.get("host").map(String::as_str) != Some("localhost:8765")
    {
        response(&mut stream, "403 Forbidden", "text/plain", "Host rejected");
        return;
    }
    if method == "POST"
        && headers
            .get("origin")
            .is_some_and(|o| o != "http://127.0.0.1:8765" && o != "http://localhost:8765")
    {
        response(
            &mut stream,
            "403 Forbidden",
            "text/plain",
            "Origin rejected",
        );
        return;
    }
    if method == "POST"
        && headers
            .get("content-type")
            .map(|s| s.split(';').next().unwrap_or("").trim())
            != Some("application/json")
    {
        response(
            &mut stream,
            "415 Unsupported Media Type",
            "text/plain",
            "Expected application/json",
        );
        return;
    }
    let len = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if len > 1_000_000 {
        return;
    }
    while data.len() < end + len {
        let mut b = [0; 4096];
        let Ok(n) = stream.read(&mut b) else { return };
        if n == 0 {
            return;
        }
        data.extend_from_slice(&b[..n]);
    }
    let body = String::from_utf8_lossy(&data[end..end + len]);
    let result: Result<String, String> = (|| match (method, path) {
        ("GET", "/") => {
            response(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                include_str!("../web/index.html"),
            );
            Ok(String::new())
        }
        ("GET", "/app.js") => {
            response(
                &mut stream,
                "200 OK",
                "text/javascript; charset=utf-8",
                include_str!("../web/app.js"),
            );
            Ok(String::new())
        }
        ("GET", "/style.css") => {
            response(
                &mut stream,
                "200 OK",
                "text/css",
                include_str!("../web/style.css"),
            );
            Ok(String::new())
        }
        ("GET", "/api/config") => Ok(state.lock().unwrap().config.encode()),
        ("GET", "/api/status") => {
            let s = state.lock().unwrap();
            let found = linux::devices();
            let detection_message = linux::detection_message(&found);
            let devices = found
                .iter()
                .map(|(p, n)| {
                    let error = linux::access_error(p);
                    format!(
                        "{{\"path\":{},\"name\":{},\"readable\":{},\"access_error\":{}}}",
                        quote(p),
                        quote(n),
                        error.is_none(),
                        quote(error.as_deref().unwrap_or(""))
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            Ok(format!(
                "{{\"auto_connect\":{},\"preferred_device\":{},\"running\":{},\"device\":{},\"error\":{},\"last_key\":{},\"sequence\":{},\"devices\":[{}],\"detection_message\":{},\"firmware_supported\":false}}",
                s.auto_connect,
                quote(&linux::preferred_device(&found).unwrap_or_default()),
                s.running,
                quote(&s.device),
                quote(&s.error),
                s.last_key,
                s.sequence,
                devices,
                quote(&detection_message)
            ))
        }
        ("POST", "/api/config") => {
            let c = parse(&body)?;
            validate(&c)?;
            let mut s = state.lock().unwrap();
            let temp = s.config_path.with_extension("json.tmp");
            fs::write(&temp, c.encode()).map_err(|e| e.to_string())?;
            fs::rename(&temp, &s.config_path).map_err(|e| e.to_string())?;
            s.config = c;
            Ok("{\"ok\":true}".into())
        }
        ("POST", "/api/start") => {
            let c = parse(&body)?;
            tx.send(Command::Start(c.get("device").str().into()))
                .map_err(|e| e.to_string())?;
            Ok("{\"ok\":true}".into())
        }
        ("POST", "/api/learn") => {
            let body = parse(&body)?;
            let mut s = state.lock().unwrap();
            let enabled = body.get("enabled") == &Json::Bool(true);
            if enabled && !s.running {
                return Err("Сначала включите перехват".into());
            }
            s.learning = enabled;
            Ok("{\"ok\":true}".into())
        }
        ("POST", "/api/auto") => {
            tx.send(Command::Auto).map_err(|e| e.to_string())?;
            Ok("{\"ok\":true}".into())
        }
        ("POST", "/api/stop") => {
            tx.send(Command::Stop).map_err(|e| e.to_string())?;
            Ok("{\"ok\":true}".into())
        }
        _ => Err("Маршрут не найден".into()),
    })();
    match result {
        Ok(s) if !s.is_empty() => {
            response(&mut stream, "200 OK", "application/json; charset=utf-8", &s)
        }
        Ok(_) => {}
        Err(e) => response(
            &mut stream,
            "400 Bad Request",
            "application/json; charset=utf-8",
            &format!("{{\"error\":{}}}", quote(&e)),
        ),
    }
}
fn main() {
    if std::env::args().any(|a| a == "--diagnose") {
        let devices = linux::devices();
        println!("{}", linux::detection_message(&devices));
        for (path, name) in &devices {
            println!(
                "{path} · {name} · {}",
                linux::access_error(path).unwrap_or_else(|| "доступно для чтения".into())
            );
        }
        println!(
            "/dev/uinput: {}",
            if std::path::Path::new("/dev/uinput").exists() {
                "существует"
            } else {
                "отсутствует"
            }
        );
        return;
    }
    let config = match fs::read_to_string("config.json") {
        Ok(s) => match parse(&s).and_then(|c| {
            validate(&c)?;
            Ok(c)
        }) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Ошибка config.json: {e}");
                std::process::exit(1)
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => parse(DEFAULT).unwrap(),
        Err(e) => {
            eprintln!("Ошибка чтения config.json: {e}");
            std::process::exit(1)
        }
    };
    let state = Arc::new(Mutex::new(State {
        config,
        config_path: PathBuf::from("config.json"),
        running: false,
        auto_connect: !std::env::args().any(|a| a == "--no-auto"),
        learning: false,
        device: String::new(),
        error: String::new(),
        last_key: 0,
        sequence: 0,
    }));
    let (tx, rx) = mpsc::channel();
    let s = state.clone();
    thread::spawn(move || worker(s, rx));
    let listener = TcpListener::bind("127.0.0.1:8765").unwrap_or_else(|e| {
        eprintln!("Не удалось открыть 127.0.0.1:8765: {e}");
        std::process::exit(1)
    });
    println!("KeyMasterBind → http://127.0.0.1:8765");
    for stream in listener.incoming().flatten() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
        serve(stream, state.clone(), tx.clone());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_validation() {
        assert!(validate(&parse(DEFAULT).unwrap()).is_ok());
        assert!(validate(&parse(r#"{"active":"a","profiles":[{"name":"a","bindings":{"1":{"kind":"chord","keys":[9999]}}}]}"#).unwrap()).is_err());
    }
    #[test]
    fn macro_limit() {
        let c=parse(r#"{"active":"a","profiles":[{"name":"a","bindings":{"1":{"kind":"macro","steps":[{"keys":[29,46],"delay":2001}]}}}]}"#).unwrap();
        assert!(validate(&c).is_err());
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::io::{self, Cursor};
    struct Memory {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Memory {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.input.read(b)
        }
    }
    impl Write for Memory {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn request(
        s: &Arc<Mutex<State>>,
        tx: &mpsc::Sender<Command>,
        method: &str,
        path: &str,
        headers: &str,
        body: &str,
    ) -> String {
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:8765\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        );
        let mut stream = Memory {
            input: Cursor::new(req.into_bytes()),
            output: vec![],
        };
        serve(&mut stream, s.clone(), tx.clone());
        String::from_utf8(stream.output).unwrap()
    }
    #[test]
    fn routes_persistence_and_request_guards() {
        let dir =
            std::env::temp_dir().join(format!("keymasterbind-rust-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let s = Arc::new(Mutex::new(State {
            config: parse(DEFAULT).unwrap(),
            config_path: path.clone(),
            running: false,
            auto_connect: true,
            learning: false,
            device: String::new(),
            error: String::new(),
            last_key: 0,
            sequence: 0,
        }));
        let (tx, rx) = mpsc::channel();
        for route in ["/", "/app.js", "/style.css", "/api/config", "/api/status"] {
            assert!(
                request(&s, &tx, "GET", route, "", "").starts_with("HTTP/1.1 200"),
                "{route}"
            );
        }
        let content = "Content-Type: application/json\r\n";
        assert!(
            request(&s, &tx, "POST", "/api/config", content, DEFAULT).starts_with("HTTP/1.1 200")
        );
        assert_eq!(
            parse(&fs::read_to_string(&path).unwrap()).unwrap(),
            parse(DEFAULT).unwrap()
        );
        for invalid in [
            "{",
            r#"{"active":"a","profiles":[]}"#,
            r#"{"active":"a","profiles":[{"name":"a","layout":[1,1],"bindings":{}}]}"#,
        ] {
            assert!(
                request(&s, &tx, "POST", "/api/config", content, invalid)
                    .starts_with("HTTP/1.1 400")
            );
        }
        assert!(
            request(
                &s,
                &tx,
                "POST",
                "/api/config",
                "Content-Type: text/plain\r\n",
                DEFAULT
            )
            .starts_with("HTTP/1.1 415")
        );
        assert!(
            request(
                &s,
                &tx,
                "POST",
                "/api/config",
                "Content-Type: application/json\r\nOrigin: https://example.com\r\n",
                DEFAULT
            )
            .starts_with("HTTP/1.1 403")
        );
        assert!(
            request(
                &s,
                &tx,
                "GET",
                "/api/status",
                "Host: attacker.invalid\r\n",
                ""
            )
            .starts_with("HTTP/1.1 403")
        );
        assert!(
            request(
                &s,
                &tx,
                "POST",
                "/api/learn",
                content,
                r#"{"enabled":true}"#
            )
            .starts_with("HTTP/1.1 400")
        );
        assert!(request(&s, &tx, "POST", "/api/stop", content, "{}").starts_with("HTTP/1.1 200"));
        assert!(matches!(rx.try_recv(), Ok(Command::Stop)));
        assert_eq!(
            parse(&fs::read_to_string(&path).unwrap()).unwrap(),
            parse(DEFAULT).unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
