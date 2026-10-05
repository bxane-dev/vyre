//! Local Steam library discovery. Installed game names and IDs come from the
//! Steam client manifests; no account login, cloud inventory, or web scraping.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

const MAX_VDF_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteamGameInstall {
    pub app_id: u32,
    pub name: String,
    pub install_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Value {
    String(String),
    Object(HashMap<String, Value>),
}

#[derive(Debug, PartialEq, Eq)]
enum Token {
    String(String),
    Open,
    Close,
}

pub fn discover_installed_games(steam_executable: Option<PathBuf>) -> Vec<SteamGameInstall> {
    let mut libraries = Vec::new();
    if let Some(path) = registry_steam_path() {
        libraries.push(path);
    }
    for variable in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(variable) {
            let base = PathBuf::from(base);
            match variable {
                "ProgramFiles(x86)" | "ProgramFiles" => libraries.push(base.join("Steam")),
                _ => libraries.push(base.join("Programs/Steam")),
            }
        }
    }
    if let Some(executable) = steam_executable {
        if executable
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("steam.exe"))
        {
            if let Some(parent) = executable.parent() {
                libraries.push(parent.to_path_buf());
            }
        }
    }

    let roots = canonical_deduplicate(libraries);
    let mut game_libraries = Vec::new();
    for root in roots {
        game_libraries.push(root.clone());
        let folders = root.join("steamapps/libraryfolders.vdf");
        if let Some(value) =
            read_vdf(&folders).and_then(|document| document.get("libraryfolders").cloned())
        {
            if let Value::Object(entries) = value {
                for entry in entries.into_values() {
                    let path = match entry {
                        Value::String(path) => Some(path),
                        Value::Object(fields) => match fields.get("path") {
                            Some(Value::String(path)) => Some(path.clone()),
                            _ => None,
                        },
                    };
                    if let Some(path) = path {
                        game_libraries.push(PathBuf::from(path));
                    }
                }
            }
        }
    }

    let mut games = Vec::new();
    for library in canonical_deduplicate(game_libraries) {
        let steamapps = library.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&steamapps) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(app_id) = filename
                .strip_prefix("appmanifest_")
                .and_then(|name| name.strip_suffix(".acf"))
                .and_then(|id| id.parse::<u32>().ok())
            else {
                continue;
            };
            let Some(document) = read_vdf(&path) else {
                continue;
            };
            let Some(Value::Object(app)) = document.get("appstate") else {
                continue;
            };
            let Some(name) = string_field(app, "name") else {
                continue;
            };
            let Some(install_dir) = string_field(app, "installdir") else {
                continue;
            };
            // App manifests supply a single folder name. Refuse traversal or
            // absolute paths before joining it to the library's common folder.
            let component = Path::new(install_dir);
            if component.components().count() != 1
                || component.file_name().is_none()
                || install_dir == "."
                || install_dir == ".."
            {
                continue;
            }
            let install_path = steamapps.join("common").join(component);
            if !install_path.is_dir() {
                continue;
            }
            let install_path = std::fs::canonicalize(&install_path).unwrap_or(install_path);
            games.push(SteamGameInstall {
                app_id,
                name: name.to_owned(),
                install_path,
            });
        }
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games.dedup_by_key(|game| game.app_id);
    games
}

pub fn game_for_executable<'a>(
    path: &Path,
    games: &'a [SteamGameInstall],
) -> Option<&'a SteamGameInstall> {
    let executable = folded_path(path);
    games.iter().find(|game| {
        let root = folded_path(&game.install_path);
        executable
            .strip_prefix(&root)
            .is_some_and(|tail| tail.starts_with('/'))
    })
}

fn normalized(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    folded_path(&canonical)
}

fn folded_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn canonical_deduplicate(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for path in paths {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let key = normalized(&path);
        if !result
            .iter()
            .any(|known: &PathBuf| normalized(known) == key)
        {
            result.push(path);
        }
    }
    result
}

fn read_vdf(path: &Path) -> Option<HashMap<String, Value>> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_VDF_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    parse_vdf(&text).ok()
}

fn string_field<'a>(object: &'a HashMap<String, Value>, key: &str) -> Option<&'a str> {
    match object.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

fn registry_steam_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::ptr;
        use windows_sys::Win32::System::Registry::{
            RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ,
        };
        let subkey: Vec<u16> = "Software\\Valve\\Steam\0".encode_utf16().collect();
        let value: Vec<u16> = "SteamPath\0".encode_utf16().collect();
        let mut bytes = 0u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut bytes,
            )
        };
        if status != 0 || bytes < 2 || bytes > 32_768 {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != 0 {
            return None;
        }
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        let path = String::from_utf16(&buffer[..end]).ok()?;
        return Some(PathBuf::from(path.replace('/', "\\")));
    }
    #[cfg(not(windows))]
    None
}

fn parse_vdf(text: &str) -> Result<HashMap<String, Value>, ()> {
    if text.len() > MAX_VDF_BYTES as usize {
        return Err(());
    }
    let tokens = tokenize(text)?;
    let mut index = 0;
    let root = parse_object(&tokens, &mut index, 0, false)?;
    if index == tokens.len() {
        Ok(root)
    } else {
        Err(())
    }
}

fn tokenize(text: &str) -> Result<Vec<Token>, ()> {
    let mut chars = text.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(ch) = chars.next() {
        match ch {
            c if c.is_whitespace() => {}
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '"' => {
                let mut value = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => match chars.next() {
                            Some('"') => value.push('"'),
                            Some('\\') => value.push('\\'),
                            Some(other) => {
                                value.push('\\');
                                value.push(other);
                            }
                            None => return Err(()),
                        },
                        other => value.push(other),
                    }
                }
                if !closed {
                    return Err(());
                }
                tokens.push(Token::String(value));
            }
            other => {
                let mut value = String::from(other);
                while let Some(next) = chars.peek() {
                    if next.is_whitespace() || *next == '{' || *next == '}' {
                        break;
                    }
                    value.push(chars.next().ok_or(())?);
                }
                tokens.push(Token::String(value));
            }
        }
        if tokens.len() > 100_000 {
            return Err(());
        }
    }
    Ok(tokens)
}

fn parse_object(
    tokens: &[Token],
    index: &mut usize,
    depth: usize,
    nested: bool,
) -> Result<HashMap<String, Value>, ()> {
    if depth > 32 {
        return Err(());
    }
    let mut object = HashMap::new();
    loop {
        match tokens.get(*index) {
            None if !nested => break,
            Some(Token::Close) if nested => {
                *index += 1;
                break;
            }
            Some(Token::String(key)) => {
                let key = key.to_ascii_lowercase();
                *index += 1;
                let value = match tokens.get(*index) {
                    Some(Token::String(value)) => {
                        *index += 1;
                        Value::String(value.clone())
                    }
                    Some(Token::Open) => {
                        *index += 1;
                        Value::Object(parse_object(tokens, index, depth + 1, true)?)
                    }
                    _ => return Err(()),
                };
                object.insert(key, value);
            }
            None | Some(Token::Close) => return Err(()),
            _ => return Err(()),
        }
    }
    Ok(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_steam_library_layout_and_escapes() {
        let document = parse_vdf(r#""libraryfolders" { "0" { "path" "C:\\Steam" "apps" { "730" "1" } } "1" { "path" "D:\\Games" } }"#).unwrap();
        let Some(Value::Object(libraries)) = document.get("libraryfolders") else {
            panic!("libraryfolders missing")
        };
        let Some(Value::Object(primary)) = libraries.get("0") else {
            panic!("primary library missing")
        };
        assert_eq!(string_field(primary, "path"), Some("C:\\Steam"));
    }

    #[test]
    fn rejects_malformed_vdf_and_only_matches_paths_inside_a_game_install() {
        assert!(parse_vdf("\"appstate\" { \"name\" \"unterminated }").is_err());
        let install = std::env::temp_dir().join("vyre-steam-game-install");
        let game = SteamGameInstall {
            app_id: 730,
            name: "Counter-Strike 2".into(),
            install_path: install.clone(),
        };
        let inside = install.join("game/bin/cs2.exe");
        let neighbor = install
            .with_file_name("vyre-steam-game-install-other")
            .join("game.exe");
        assert_eq!(
            game_for_executable(&inside, std::slice::from_ref(&game)).map(|g| g.app_id),
            Some(730)
        );
        assert_eq!(game_for_executable(&neighbor, &[game]), None);
    }
}
