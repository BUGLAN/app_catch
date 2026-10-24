//! 文件系统小工具：原子写盘。
//!
//! `AppStore` 用它保证「要么看到完整旧文件，要么看到完整新文件」，避免进程被中断时留下半截 TOML。
//! 实现策略：同目录临时文件 → 写入并 flush → `rename` 覆盖目标。
//! 临时文件必须与目标同目录，跨盘 rename 不具备原子性。

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::error::{AppCatchError, Result};

/// 原子地把 `contents` 写入 `target`（覆盖已存在的文件）。
///
/// 步骤：在同目录创建唯一临时文件 `<name>.tmp-<pid>-<nanos>` → 写入 → `sync_all` → `rename`。
/// 任一环节失败都会清理临时文件并返回 `AppCatchError::Io`。
pub fn write_atomic(target: &Path, contents: &str) -> Result<()> {
    let parent = target.parent().ok_or_else(|| {
        AppCatchError::Io {
            path: target.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "目标路径没有父目录，无法原子写入",
            ),
        }
    })?;

    fs::create_dir_all(parent).map_err(|e| AppCatchError::io(parent, e))?;

    let file_name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "tmp".to_string());
    // 用 pid + 纳秒计数保证同一目录下并发写入不会撞名
    let unique = format!(
        "{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let tmp_path = parent.join(unique);

    let write_result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        // 清理失败不覆盖原始错误，忽略清理错误即可
        let _ = fs::remove_file(&tmp_path);
        return Err(AppCatchError::io(&tmp_path, e));
    }

    if let Err(e) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(AppCatchError::io(target, e));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn write_atomic_creates_and_overwrites() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let target = dir.path().join("a.txt");

        write_atomic(&target, "first").expect("首次写入应成功");
        assert_eq!(fs::read_to_string(&target).expect("读取失败"), "first");

        write_atomic(&target, "second").expect("覆盖写入应成功");
        assert_eq!(fs::read_to_string(&target).expect("读取失败"), "second");

        // 目录里不应残留临时文件
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("读取目录失败")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件，实际: {leftovers:?}");
    }

    #[test]
    fn write_atomic_writes_utf8_content_readable_back() {
        let dir = tempfile::tempdir().expect("创建临时目录失败");
        let target = dir.path().join("nested/dir/b.txt");
        write_atomic(&target, "中文内容\n第二行").expect("写入失败");

        let mut buf = String::new();
        fs::File::open(&target)
            .expect("打开失败")
            .read_to_string(&mut buf)
            .expect("读取失败");
        assert_eq!(buf, "中文内容\n第二行");
    }
}
