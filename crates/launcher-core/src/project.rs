//! 项目类型识别：根据工作目录里的清单文件推断「怎么启动它」。
//!
//! 只读文件、不执行任何命令，任何平台都能跑、都能单测。
//! 产出的是**候选**而不是结论：界面把候选列出来让用户点选，
//! 不会静默覆盖用户已经填好的启动命令。
//!
//! 当前识别：
//! - Node：`package.json` 的 `scripts`，包管理器按 `packageManager` 字段 > 锁文件 > npm 推断；
//! - Python：`uv.lock` / `.venv` / `venv` + 常见入口文件（main.py / app.py / webui.py / manage.py…）；
//! - Rust：`Cargo.toml` → `cargo run`；
//! - Go：`go.mod` → `go run .`；
//! - Docker Compose：`compose.yaml` / `docker-compose.yml` → `docker compose up`；
//! - 根目录启动脚本：`start.sh` / `run.sh` / `start.bat` / `run.bat` 等。

use std::fs;
use std::path::Path;

use serde::Serialize;

/// 一条启动候选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandSuggestion {
    /// 界面展示用的短标签，例如 `dev`、`main.py`。
    pub label: String,
    /// 可执行程序。
    pub command: String,
    /// 参数（每项一个，不做空白拆分）。
    pub args: Vec<String>,
    /// 候选出处，例如 `package.json scripts.dev`。
    pub source: String,
}

/// 识别结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct ProjectDetection {
    /// 识别到的项目类型（可能多种并存，例如 Node + Docker），按优先级排列。
    pub kinds: Vec<String>,
    /// 清单文件里声明的项目名（例如 package.json 的 `name`），用于预填名称。
    pub name: Option<String>,
    /// 启动候选，第一条是最推荐的。
    pub suggestions: Vec<CommandSuggestion>,
}

/// 识别目录下的项目类型并给出启动候选。目录不存在或不可读时返回空结果。
pub fn detect_project(dir: &Path) -> ProjectDetection {
    let mut out = ProjectDetection::default();
    if !dir.is_dir() {
        return out;
    }
    detect_node(dir, &mut out);
    detect_python(dir, &mut out);
    detect_rust(dir, &mut out);
    detect_go(dir, &mut out);
    detect_compose(dir, &mut out);
    detect_scripts(dir, &mut out);
    if out.name.is_none() {
        out.name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty());
    }
    out
}

/// Windows 上 npm / pnpm / yarn 是 `.cmd` 包装脚本，`CreateProcess` 不会自动补扩展名。
fn shim(program: &str) -> String {
    if cfg!(windows) {
        format!("{program}.cmd")
    } else {
        program.to_string()
    }
}

fn suggestion(label: &str, command: String, args: &[&str], source: String) -> CommandSuggestion {
    CommandSuggestion {
        label: label.to_string(),
        command,
        args: args.iter().map(|arg| arg.to_string()).collect(),
        source,
    }
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// 常见「启动类」脚本，优先级从高到低；不在表里的脚本排在后面、保持原顺序。
const NODE_SCRIPT_PRIORITY: &[&str] = &["dev", "start", "serve", "develop", "preview", "watch"];

/// 包管理器在安装 / 打包 / 发布时自动触发的生命周期脚本。
const NODE_LIFECYCLE_SCRIPTS: &[&str] = &[
    "preinstall",
    "install",
    "postinstall",
    "prepare",
    "prepublish",
    "prepublishOnly",
    "prepack",
    "postpack",
    "publish",
    "postpublish",
];

/// 推断包管理器：`packageManager` 字段 > 锁文件 > npm。
fn node_package_manager(dir: &Path, manifest: &serde_json::Value) -> &'static str {
    if let Some(field) = manifest.get("packageManager").and_then(|v| v.as_str()) {
        let name = field.split('@').next().unwrap_or("");
        match name {
            "pnpm" => return "pnpm",
            "yarn" => return "yarn",
            "bun" => return "bun",
            "npm" => return "npm",
            _ => {}
        }
    }
    if dir.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if dir.join("yarn.lock").is_file() {
        "yarn"
    } else if dir.join("bun.lockb").is_file() || dir.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    }
}

fn detect_node(dir: &Path, out: &mut ProjectDetection) {
    let Ok(text) = fs::read_to_string(dir.join("package.json")) else {
        return;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let pm = node_package_manager(dir, &manifest);
    out.kinds.push(format!("node:{pm}"));
    if let Some(name) = manifest.get("name").and_then(|v| v.as_str()) {
        // scoped 包名（@scope/pkg）只取最后一段做显示名
        let short = name.rsplit('/').next().unwrap_or(name).trim();
        if !short.is_empty() {
            out.name = Some(short.to_string());
        }
    }
    // bun 本身就是可执行文件，不需要 .cmd
    let program = if pm == "bun" {
        pm.to_string()
    } else {
        shim(pm)
    };

    let mut scripts: Vec<&str> = manifest
        .get("scripts")
        .and_then(|v| v.as_object())
        .map(|map| map.keys().map(String::as_str).collect())
        .unwrap_or_default();
    // 稳定排序：优先表里的按表顺序，其余保持 package.json 中的顺序
    scripts.sort_by_key(|name| {
        NODE_SCRIPT_PRIORITY
            .iter()
            .position(|p| p == name)
            .unwrap_or(NODE_SCRIPT_PRIORITY.len())
    });
    for name in scripts {
        // 安装 / 发布生命周期脚本不是「启动」入口
        if NODE_LIFECYCLE_SCRIPTS.contains(&name) {
            continue;
        }
        // pre/post 钩子脚本由包管理器自动触发，单独列出来没有意义
        if name.starts_with("pre") || name.starts_with("post") {
            let base = name.trim_start_matches("pre").trim_start_matches("post");
            if manifest["scripts"].get(base).is_some() {
                continue;
            }
        }
        out.suggestions.push(suggestion(
            name,
            program.clone(),
            &["run", name],
            format!("package.json scripts.{name}"),
        ));
    }
    // 没有 scripts 但声明了 main：直接 node 跑入口
    if !out
        .suggestions
        .iter()
        .any(|s| s.source.starts_with("package.json"))
    {
        if let Some(main) = manifest.get("main").and_then(|v| v.as_str()) {
            out.suggestions.push(suggestion(
                main,
                "node".to_string(),
                &[main],
                "package.json main".to_string(),
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Python
// ---------------------------------------------------------------------------

const PYTHON_ENTRIES: &[&str] = &[
    "main.py",
    "app.py",
    "webui.py",
    "server.py",
    "run.py",
    "manage.py",
];

/// 项目自带虚拟环境里的解释器（相对路径，配合工作目录使用）。
fn python_venv(dir: &Path) -> Option<String> {
    for venv in [".venv", "venv", "env"] {
        let rel = if cfg!(windows) {
            format!("{venv}/Scripts/python.exe")
        } else {
            format!("{venv}/bin/python")
        };
        if dir.join(&rel).is_file() {
            // 启动时 program 不按 cwd 解析，给绝对路径最稳
            return Some(dir.join(&rel).to_string_lossy().replace('\\', "/"));
        }
    }
    None
}

fn detect_python(dir: &Path, out: &mut ProjectDetection) {
    let markers = [
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        "uv.lock",
        "Pipfile",
    ];
    let has_marker = markers.iter().any(|m| dir.join(m).is_file());
    let entries: Vec<&str> = PYTHON_ENTRIES
        .iter()
        .copied()
        .filter(|entry| dir.join(entry).is_file())
        .collect();
    if !has_marker && entries.is_empty() {
        return;
    }
    out.kinds.push("python".to_string());

    let uv = dir.join("uv.lock").is_file();
    let interpreter = python_venv(dir)
        .unwrap_or_else(|| if cfg!(windows) { "python" } else { "python3" }.to_string());
    for entry in entries {
        let mut args: Vec<&str> = Vec::new();
        let command = if uv {
            args.extend(["run", "python"]);
            "uv".to_string()
        } else {
            interpreter.clone()
        };
        args.push(entry);
        if entry == "manage.py" {
            args.push("runserver");
        }
        let source = if uv {
            format!("uv.lock + {entry}")
        } else {
            entry.to_string()
        };
        out.suggestions
            .push(suggestion(entry, command, &args, source));
    }
}

// ---------------------------------------------------------------------------
// Rust / Go / Compose / 脚本
// ---------------------------------------------------------------------------

fn detect_rust(dir: &Path, out: &mut ProjectDetection) {
    let Ok(text) = fs::read_to_string(dir.join("Cargo.toml")) else {
        return;
    };
    out.kinds.push("rust".to_string());
    if out.name.is_none() {
        if let Ok(value) = text.parse::<toml::Table>() {
            if let Some(name) = value
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                out.name = Some(name.to_string());
            }
        }
    }
    out.suggestions.push(suggestion(
        "cargo run",
        "cargo".into(),
        &["run"],
        "Cargo.toml".into(),
    ));
    out.suggestions.push(suggestion(
        "cargo run --release",
        "cargo".into(),
        &["run", "--release"],
        "Cargo.toml".into(),
    ));
}

fn detect_go(dir: &Path, out: &mut ProjectDetection) {
    if !dir.join("go.mod").is_file() {
        return;
    }
    out.kinds.push("go".to_string());
    out.suggestions.push(suggestion(
        "go run .",
        "go".into(),
        &["run", "."],
        "go.mod".into(),
    ));
}

fn detect_compose(dir: &Path, out: &mut ProjectDetection) {
    let files = [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ];
    let Some(file) = files.iter().find(|f| dir.join(f).is_file()) else {
        return;
    };
    out.kinds.push("docker-compose".to_string());
    out.suggestions.push(suggestion(
        "docker compose up",
        "docker".into(),
        &["compose", "up"],
        (*file).to_string(),
    ));
}

fn detect_scripts(dir: &Path, out: &mut ProjectDetection) {
    let candidates: &[&str] = if cfg!(windows) {
        &[
            "start.bat",
            "run.bat",
            "start.cmd",
            "run.cmd",
            "go-webui.bat",
            "start.ps1",
        ]
    } else {
        &["start.sh", "run.sh", "dev.sh", "go-webui.sh"]
    };
    for name in candidates {
        let path = dir.join(name);
        if !path.is_file() {
            continue;
        }
        let abs = path.to_string_lossy().replace('\\', "/");
        let (command, args): (String, Vec<&str>) = if name.ends_with(".sh") {
            ("bash".into(), vec![name])
        } else if name.ends_with(".ps1") {
            (
                "powershell".into(),
                vec!["-ExecutionPolicy", "Bypass", "-File", name],
            )
        } else {
            (abs, vec![])
        };
        out.suggestions
            .push(suggestion(name, command, &args, format!("脚本 {name}")));
    }
    if !out.suggestions.is_empty() && out.kinds.is_empty() {
        out.kinds.push("script".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn missing_dir_yields_empty() {
        let result = detect_project(Path::new("/definitely/not/here/xyz"));
        assert!(result.kinds.is_empty());
        assert!(result.suggestions.is_empty());
        assert!(result.name.is_none());
    }

    #[test]
    fn node_scripts_sorted_with_dev_first_and_pnpm_from_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "package.json",
            r#"{"name":"@acme/web","scripts":{"build":"vite build","lint":"x","prebuild":"y","dev":"vite","start":"node ."}}"#,
        );
        write(tmp.path(), "pnpm-lock.yaml", "");
        let result = detect_project(tmp.path());
        assert_eq!(result.kinds, vec!["node:pnpm"]);
        assert_eq!(result.name.as_deref(), Some("web"));
        let labels: Vec<&str> = result
            .suggestions
            .iter()
            .map(|s| s.label.as_str())
            .collect();
        assert_eq!(labels, vec!["dev", "start", "build", "lint"]);
        assert_eq!(result.suggestions[0].command, shim("pnpm"));
        assert_eq!(result.suggestions[0].args, vec!["run", "dev"]);
    }

    #[test]
    fn package_manager_field_wins_over_lockfile() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "package.json",
            r#"{"packageManager":"yarn@4.1.0","scripts":{"dev":"x"}}"#,
        );
        write(tmp.path(), "package-lock.json", "{}");
        let result = detect_project(tmp.path());
        assert_eq!(result.kinds, vec!["node:yarn"]);
        assert_eq!(result.suggestions[0].command, shim("yarn"));
    }

    #[test]
    fn python_entry_with_venv_and_django() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "requirements.txt", "django");
        write(tmp.path(), "manage.py", "");
        let venv_python = if cfg!(windows) {
            ".venv/Scripts/python.exe"
        } else {
            ".venv/bin/python"
        };
        write(tmp.path(), venv_python, "");
        let result = detect_project(tmp.path());
        assert_eq!(result.kinds, vec!["python"]);
        let s = &result.suggestions[0];
        assert!(s.command.ends_with(venv_python), "{}", s.command);
        assert_eq!(s.args, vec!["manage.py", "runserver"]);
    }

    #[test]
    fn uv_project_uses_uv_run() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "uv.lock", "");
        write(tmp.path(), "main.py", "");
        let result = detect_project(tmp.path());
        assert_eq!(result.suggestions[0].command, "uv");
        assert_eq!(result.suggestions[0].args, vec!["run", "python", "main.py"]);
    }

    #[test]
    fn rust_and_go_and_compose() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "Cargo.toml", "[package]\nname = \"demo\"\n");
        write(tmp.path(), "go.mod", "module x");
        write(tmp.path(), "compose.yaml", "");
        let result = detect_project(tmp.path());
        assert_eq!(result.kinds, vec!["rust", "go", "docker-compose"]);
        assert_eq!(result.name.as_deref(), Some("demo"));
        assert_eq!(result.suggestions[0].command, "cargo");
    }

    #[test]
    fn falls_back_to_dir_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("my-tool");
        fs::create_dir(&dir).unwrap();
        let result = detect_project(&dir);
        assert_eq!(result.name.as_deref(), Some("my-tool"));
        assert!(result.suggestions.is_empty());
    }
}
