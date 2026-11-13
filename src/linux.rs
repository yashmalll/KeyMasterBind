use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    mem::size_of,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};
unsafe extern "C" {
    fn ioctl(fd: i32, request: usize, ...) -> i32;
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Event {
    pub sec: isize,
    pub usec: isize,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}
#[repr(C)]
#[derive(Default)]
struct Id {
    bus: u16,
    vendor: u16,
    product: u16,
    version: u16,
}
#[repr(C)]
struct Setup {
    id: Id,
    name: [u8; 80],
    ff: u32,
}
fn ctl(file: &File, req: usize, arg: usize) -> io::Result<()> {
    if unsafe { ioctl(file.as_raw_fd(), req, arg) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
pub fn devices() -> Vec<(String, String)> {
    discover(
        std::path::Path::new("/sys/class/input"),
        &fs::read_to_string("/proc/bus/input/devices").unwrap_or_default(),
    )
}
fn discover(sys_root: &std::path::Path, proc_text: &str) -> Vec<(String, String)> {
    let mut out = std::collections::BTreeMap::new();
    if let Ok(entries) = fs::read_dir(sys_root) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !event_name(&n) {
                continue;
            }
            let p = e.path().join("device");
            let vendor = fs::read_to_string(p.join("id/vendor")).unwrap_or_default();
            let product = fs::read_to_string(p.join("id/product")).unwrap_or_default();
            if hex_id(&vendor) == Some(0x30fa) && hex_id(&product) == Some(0x2350) {
                out.insert(
                    format!("/dev/input/{n}"),
                    fs::read_to_string(p.join("name"))
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                );
            }
        }
    }
    for block in proc_text.split("\n\n") {
        let mut vendor = None;
        let mut product = None;
        let mut name = String::new();
        let mut events = Vec::new();
        for line in block.lines() {
            if let Some(ids) = line.strip_prefix("I: ") {
                for token in ids.split_whitespace() {
                    if let Some(v) = token.strip_prefix("Vendor=") {
                        vendor = hex_id(v);
                    }
                    if let Some(p) = token.strip_prefix("Product=") {
                        product = hex_id(p);
                    }
                }
            } else if let Some(n) = line.strip_prefix("N: Name=\"") {
                name = n.strip_suffix('"').unwrap_or(n).to_string();
            } else if let Some(handlers) = line.strip_prefix("H: Handlers=") {
                events.extend(handlers.split_whitespace().filter(|n| event_name(n)));
            }
        }
        if vendor == Some(0x30fa) && product == Some(0x2350) {
            for event in events {
                out.entry(format!("/dev/input/{event}"))
                    .or_insert_with(|| name.clone());
            }
        }
    }
    let mut out: Vec<_> = out.into_iter().collect();
    out.sort_by_key(|(p, n)| (n != "INSTANT USB Keyboard", p.clone()));
    out
}
pub fn preferred_device(devices: &[(String, String)]) -> Option<String> {
    devices
        .iter()
        .filter(|(_, name)| name == "INSTANT USB Keyboard")
        .min_by_key(|(path, _)| {
            path.strip_prefix("/dev/input/event")
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(u32::MAX)
        })
        .map(|(path, _)| path.clone())
}
fn hex_id(s: &str) -> Option<u16> {
    u16::from_str_radix(s.trim(), 16).ok()
}
fn event_name(n: &str) -> bool {
    n.strip_prefix("event")
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
}
pub fn access_error(path: &str) -> Option<String> {
    File::open(path).err().map(|e| match e.kind() {
        io::ErrorKind::NotFound => format!("{path} недоступен в среде запуска. Запустите бинарник на хосте, вне контейнера или песочницы, с доступом к /dev/input."),
        io::ErrorKind::PermissionDenied => format!("Нет прав на {path}. Настройте группу и udev по README, затем войдите в сеанс заново."),
        _ => format!("Не удалось открыть {path}: {e}"),
    })
}
pub fn detection_message(devices: &[(String, String)]) -> String {
    if devices.is_empty() {
        return "Клавиатура 30fa:2350 не найдена через sysfs и procfs. Проверьте подключение USB и запуск приложения на хосте.".into();
    }
    if devices.iter().any(|(p, _)| access_error(p).is_none()) {
        return "Клавиатура найдена. Выберите основной клавиатурный интерфейс.".into();
    }
    format!(
        "Клавиатура обнаружена, но перехват недоступен. {}",
        access_error(&devices[0].0).unwrap_or_default()
    )
}
pub struct Input {
    file: File,
    grabbed: bool,
}
impl Input {
    pub fn open(path: &str) -> io::Result<Self> {
        Ok(Self {
            file: OpenOptions::new()
                .read(true)
                .custom_flags(0x800)
                .open(path)?,
            grabbed: false,
        })
    }
    pub fn grab(&mut self, on: bool) -> io::Result<()> {
        ctl(&self.file, 0x40044590, on as usize)?;
        self.grabbed = on;
        Ok(())
    }
    pub fn read(&mut self) -> io::Result<Option<Event>> {
        let mut e = Event::default();
        let bytes = unsafe {
            std::slice::from_raw_parts_mut((&mut e as *mut Event).cast::<u8>(), size_of::<Event>())
        };
        match self.file.read(bytes) {
            Ok(n) if n == size_of::<Event>() => Ok(Some(e)),
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Устройство отключено",
            )),
            Ok(_) => Err(io::Error::other("Неполное input_event")),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        if self.grabbed {
            let _ = self.grab(false);
        }
    }
}
pub struct Output {
    file: File,
}
impl Output {
    pub fn new() -> io::Result<Self> {
        let file = OpenOptions::new().write(true).open("/dev/uinput")?;
        ctl(&file, 0x40045564, 1)?;
        for key in 1..=767 {
            ctl(&file, 0x40045565, key)?
        }
        let mut setup = Setup {
            id: Id {
                bus: 3,
                vendor: 0x1209,
                product: 1,
                version: 1,
            },
            name: [0; 80],
            ff: 0,
        };
        let name = b"KeyMasterBind virtual keyboard";
        setup.name[..name.len()].copy_from_slice(name);
        ctl(&file, 0x405c5503, (&setup as *const Setup) as usize)?;
        ctl(&file, 0x5501, 0)?;
        Ok(Self { file })
    }
    pub fn key(&mut self, code: u16, value: i32) -> io::Result<()> {
        for e in [
            Event {
                kind: 1,
                code,
                value,
                ..Event::default()
            },
            Event::default(),
        ] {
            let bytes = unsafe {
                std::slice::from_raw_parts((&e as *const Event).cast::<u8>(), size_of::<Event>())
            };
            self.file.write_all(bytes)?
        }
        Ok(())
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let _ = ctl(&self.file, 0x5502, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PROC: &str = "I: Bus=0003 Vendor=30FA Product=2350 Version=0110\nN: Name=\"INSTANT USB Keyboard\"\nH: Handlers=sysrq kbd leds event26\n\nI: Bus=0003 Vendor=046d Product=4076 Version=0111\nN: Name=\"Other keyboard\"\nH: Handlers=kbd event4\n\nI: Bus=0003 Vendor=30fa Product=2350 Version=0110\nN: Name=\"INSTANT USB Keyboard Mouse\"\nH: Handlers=event30 mouse2\n";
    #[test]
    fn automatic_selection_ignores_mouse_and_sorts_event_numbers() {
        let devices = vec![
            ("/dev/input/event10".into(), "INSTANT USB Keyboard".into()),
            (
                "/dev/input/event1".into(),
                "INSTANT USB Keyboard Mouse".into(),
            ),
            ("/dev/input/event2".into(), "INSTANT USB Keyboard".into()),
            (
                "/dev/input/event0".into(),
                "INSTANT USB Keyboard Consumer Control".into(),
            ),
        ];
        assert_eq!(
            preferred_device(&devices).as_deref(),
            Some("/dev/input/event2")
        );
        assert_eq!(preferred_device(&devices[1..2]), None);
        assert_eq!(preferred_device(&[]), None);
        assert_eq!(
            preferred_device(&[("/dev/input/event31".into(), "INSTANT USB Keyboard".into())])
                .as_deref(),
            Some("/dev/input/event31")
        );
    }
    #[test]
    fn proc_fallback_matches_exact_ids() {
        let devices = discover(std::path::Path::new("/nonexistent-sysfs"), PROC);
        assert_eq!(
            devices,
            vec![
                ("/dev/input/event26".into(), "INSTANT USB Keyboard".into()),
                (
                    "/dev/input/event30".into(),
                    "INSTANT USB Keyboard Mouse".into()
                )
            ]
        );
    }
    #[test]
    fn sysfs_and_proc_merge_without_duplicates() {
        let root =
            std::env::temp_dir().join(format!("keymasterbind-discovery-{}", std::process::id()));
        let p = root.join("event26/device");
        fs::create_dir_all(p.join("id")).unwrap();
        fs::write(p.join("id/vendor"), "30FA\n").unwrap();
        fs::write(p.join("id/product"), "2350\n").unwrap();
        fs::write(p.join("name"), "INSTANT USB Keyboard\n").unwrap();
        assert_eq!(discover(&root, PROC).len(), 2);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn absent_nodes_explain_access_instead_of_missing_device() {
        let message = detection_message(&[(
            "/definitely-missing-keymasterbind/event1".into(),
            "INSTANT".into(),
        )]);
        assert!(message.contains("обнаружена") && message.contains("/dev/input"));
        assert!(detection_message(&[]).contains("не найдена"));
    }
}
