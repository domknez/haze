//! Self-update from GitHub Releases: find the latest release, download the file for
//! this platform, check it against the release's `SHA256SUMS.txt`, swap it in, and
//! relaunch.
//!
//! - macOS: the `.dmg`; its `haze.app` replaces the running bundle.
//! - Linux: the bare binary (`haze-<v>-linux-<arch>`); the running executable is
//!   renamed aside and the new one takes its place.
//! - Windows: the `.zip`, since ffmpeg's DLLs beside `haze.exe` change with it; each
//!   file in the folder is renamed aside (Windows allows that for a running exe and
//!   loaded DLLs) and the new one takes its place.
//!
//! Where haze can't replace itself (a dev build, a read-only location, a release
//! without a matching file) the caller opens the release page instead.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const LATEST: &str = "https://api.github.com/repos/domknez/haze/releases/latest";
const SUMS: &str = "SHA256SUMS.txt";

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    /// The release page, for manual installs.
    pub page: String,
    assets: Vec<(String, String)>,
}

impl Release {
    fn asset(&self, name: &str) -> Option<&str> {
        self.assets.iter().find(|(n, _)| n == name).map(|(_, url)| url.as_str())
    }

    /// The file this platform installs from, if the release has it.
    fn package(&self) -> Option<(String, &str)> {
        let name = package_name(&self.version)?;
        let url = self.asset(&name)?;
        Some((name, url))
    }
}

/// Where the running copy lives and how it gets replaced.
enum Target {
    /// macOS app bundle.
    Bundle(PathBuf),
    /// Executable file (Linux).
    Binary(PathBuf),
    /// Executable with the libraries beside it (Windows); the folder's files are
    /// replaced one by one.
    Folder(PathBuf),
}

impl Target {
    fn path(&self) -> &Path {
        let (Target::Bundle(p) | Target::Binary(p) | Target::Folder(p)) = self;
        p
    }
}

/// What to start once the new version is in place.
pub struct Restart(PathBuf);

fn package_name(version: &str) -> Option<String> {
    if cfg!(target_os = "macos") {
        Some(format!("haze-{version}-macos.dmg"))
    } else if cfg!(windows) {
        Some(format!("haze-{version}-windows-x64.zip"))
    } else if cfg!(target_os = "linux") {
        Some(format!("haze-{version}-linux-{}", std::env::consts::ARCH))
    } else {
        None
    }
}

fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(concat!("haze/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(timeout)
        .build()
        .into()
}

/// `a` is a newer `major.minor.patch` than `b`.
pub fn is_newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    parse(a) > parse(b)
}

/// The latest release, if it is newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let reply: serde_json::Value = agent(Some(Duration::from_secs(20)))
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .into_body()
        .read_json()
        .map_err(|e| e.to_string())?;
    let version = reply["tag_name"].as_str().ok_or("no tag in release")?.trim_start_matches('v').to_string();
    let assets = reply["assets"]
        .as_array()
        .map(|a| {
            a.iter().filter_map(|x| Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string()))).collect()
        })
        .unwrap_or_default();
    let page = reply["html_url"].as_str().unwrap_or_default().to_string();
    let release = Release { version, page, assets };
    Ok(is_newer(&release.version, CURRENT).then_some(release))
}

/// Whether [`install`] can replace this copy with `release` (otherwise: open the page).
pub fn can_install(release: &Release) -> bool {
    release.package().is_some() && release.asset(SUMS).is_some() && target().is_some_and(|t| writable(&t))
}

fn target() -> Option<Target> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    if cfg!(target_os = "macos") {
        // .../haze.app/Contents/MacOS/haze; a bare binary is a dev build.
        let bundle = exe.ancestors().nth(3)?;
        (bundle.extension()? == "app").then(|| Target::Bundle(bundle.to_path_buf()))
    } else if cfg!(windows) {
        Some(Target::Folder(exe))
    } else {
        Some(Target::Binary(exe))
    }
}

/// The replacement is renamed into place, so its folder must be writable.
fn writable(target: &Target) -> bool {
    let Some(dir) = target.path().parent() else { return false };
    let probe = dir.join(".haze-write-test");
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// A downloaded, verified update, unpacked next to the running copy and waiting
/// for [`apply`].
pub struct Staged {
    pub version: String,
    target: Target,
    path: PathBuf,
}

/// Download and verify `release`, then unpack it next to the running copy so that
/// [`apply`] only has to rename. `progress` goes 0..=1000 during the download.
pub fn stage(release: &Release, progress: &Arc<AtomicU32>) -> Result<Staged, String> {
    let target = target().ok_or("this copy of haze can't update itself")?;
    let (name, url) = release.package().ok_or("the release has no download for this platform")?;
    let sums = agent(Some(Duration::from_secs(20)))
        .get(release.asset(SUMS).ok_or("the release has no checksums")?)
        .call()
        .and_then(|r| r.into_body().read_to_string())
        .map_err(|e| format!("checksums: {e}"))?;
    let expected = expected_sha256(&sums, &name).ok_or_else(|| format!("no checksum for {name}"))?;

    let dir = std::env::temp_dir().join(format!("haze-update-{}", release.version));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(&name);
    let actual = download(url, &file, progress)?;
    if actual != expected {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("{name}: checksum mismatch"));
    }
    log_info!("update: downloaded and verified {name}");

    let path = staged_path(&target);
    let unpacked = match &target {
        Target::Bundle(_) => unpack_bundle(&file, &path, &dir),
        Target::Binary(_) => stage_binary(&file, &path),
        Target::Folder(_) => unpack_folder(&file, &path),
    };
    let _ = std::fs::remove_dir_all(&dir);
    unpacked?;
    Ok(Staged { version: release.version.clone(), target, path })
}

/// Swap the staged copy in place of the running one. Quick (renames only), so it
/// can run right before quitting. Returns what [`relaunch`] should start.
pub fn apply(staged: &Staged) -> Result<Restart, String> {
    let current = staged.target.path();
    if let Target::Folder(_) = staged.target {
        let dir = current.parent().ok_or("haze has no folder")?;
        swap_folder(&staged.path, dir)?;
        let _ = std::fs::remove_dir_all(&staged.path);
    } else {
        let old = old_path(&staged.target);
        let _ = remove(&old);
        std::fs::rename(current, &old).map_err(|e| format!("moving the old version aside: {e}"))?;
        if let Err(e) = std::fs::rename(&staged.path, current) {
            let _ = std::fs::rename(&old, current);
            return Err(format!("installing the new version: {e}"));
        }
        let _ = remove(&old);
    }
    log_info!("update: installed {}", staged.version);
    Ok(Restart(current.to_path_buf()))
}

/// [`stage`] then [`apply`] in one go (command line).
pub fn install(release: &Release, progress: &Arc<AtomicU32>) -> Result<Restart, String> {
    apply(&stage(release, progress)?)
}

/// `sha256sum` output: `<hex>  <name>` per line.
fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

/// Stream `url` into `path`; returns the file's SHA-256 (hex).
fn download(url: &str, path: &Path, progress: &Arc<AtomicU32>) -> Result<String, String> {
    let resp = agent(Some(Duration::from_secs(600))).get(url).call().map_err(|e| format!("download: {e}"))?;
    let total = resp.body().content_length().unwrap_or(0);
    let mut body = resp.into_body().into_reader();
    let mut out = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let (mut buf, mut done) = (vec![0u8; 64 * 1024], 0u64);
    loop {
        let n = body.read(&mut buf).map_err(|e| format!("download: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if let Some(permille) = (done * 1000).checked_div(total) {
            progress.store(permille.min(1000) as u32, Ordering::Relaxed);
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// macOS: copy `haze.app` out of the DMG to `dest`, next to the running bundle.
fn unpack_bundle(dmg: &Path, dest: &Path, work: &Path) -> Result<(), String> {
    let mount = work.join("mnt");
    run("hdiutil", &["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint", path_str(&mount)?, path_str(dmg)?])?;
    let _ = std::fs::remove_dir_all(dest);
    let copied = run("ditto", &[path_str(&mount.join("haze.app"))?, path_str(dest)?]);
    let _ = run("hdiutil", &["detach", "-quiet", path_str(&mount)?]);
    copied
}

/// Linux: the new executable next to the running one (which can be renamed but not
/// overwritten while it runs).
fn stage_binary(new: &Path, dest: &Path) -> Result<(), String> {
    std::fs::copy(new, dest).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Windows: the zip's folder (`haze-<v>-windows-x64/…`) unpacked to `dest`, by the
/// `tar` Windows has had since 10.
fn unpack_folder(zip: &Path, dest: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    run("tar", &["-xf", path_str(zip)?, "-C", path_str(dest)?])?;
    // Its one folder's files move up, so `dest` holds what goes beside haze.exe.
    let inner = std::fs::read_dir(dest)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .ok_or("the download has no folder")?;
    for entry in std::fs::read_dir(&inner).map_err(|e| e.to_string())?.filter_map(Result::ok) {
        std::fs::rename(entry.path(), dest.join(entry.file_name())).map_err(|e| e.to_string())?;
    }
    std::fs::remove_dir(&inner).map_err(|e| e.to_string())?;
    if !dest.join("haze.exe").is_file() {
        return Err("the download has no haze.exe".into());
    }
    Ok(())
}

/// Windows: each file in `staged` takes the place of its namesake in `dir`, which
/// is renamed to `<name>.old` first; on a failure the files moved so far go back.
fn swap_folder(staged: &Path, dir: &Path) -> Result<(), String> {
    let files: Vec<PathBuf> =
        std::fs::read_dir(staged).map_err(|e| e.to_string())?.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect();
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    for new in &files {
        let Some(name) = new.file_name() else { continue };
        let current = dir.join(name);
        let old = aside(&current);
        let _ = std::fs::remove_file(&old);
        let moved = current.exists().then(|| std::fs::rename(&current, &old).map(|_| old.clone())).transpose();
        let result = moved.and_then(|old| std::fs::rename(new, &current).map(|_| old));
        match result {
            Ok(old) => done.push((current, old)),
            Err(e) => {
                for (current, old) in done.into_iter().rev() {
                    let _ = std::fs::remove_file(&current);
                    if let Some(old) = old {
                        let _ = std::fs::rename(old, &current);
                    }
                }
                return Err(format!("installing the new version: {e}"));
            }
        }
    }
    // Files still in use can't be deleted now; `clean_up` gets them next time.
    for (_, old) in done {
        if let Some(old) = old {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(())
}

/// `haze.exe` → `haze.exe.old`.
fn aside(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

fn staged_path(target: &Target) -> PathBuf {
    match target {
        Target::Bundle(b) => b.with_file_name(".haze-update.app"),
        Target::Binary(exe) => exe.with_extension("new"),
        Target::Folder(exe) => exe.with_file_name(".haze-update"),
    }
}

fn old_path(target: &Target) -> PathBuf {
    match target {
        Target::Bundle(b) => b.with_file_name(".haze-old.app"),
        Target::Binary(exe) | Target::Folder(exe) => aside(exe),
    }
}

fn remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
}

/// Remove what an earlier session left behind: the old version (Windows can't
/// delete a running exe or loaded DLLs) and a staged update that was never applied.
pub fn clean_up() {
    let Some(target) = target() else { return };
    let _ = remove(&staged_path(&target));
    match &target {
        Target::Folder(exe) => {
            let Some(dir) = exe.parent() else { return };
            let olds = std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok).map(|e| e.path());
            for old in olds.filter(|p| p.extension().is_some_and(|e| e == "old")) {
                let _ = std::fs::remove_file(old);
            }
        }
        _ => {
            let _ = remove(&old_path(&target));
        }
    }
}

/// Start the new version once this process has exited. Call just before quitting.
pub fn relaunch(restart: &Restart) {
    let path = &restart.0;
    let spawned = if cfg!(target_os = "macos") {
        // `open` would just focus the old instance while it still runs.
        let script = format!("while kill -0 {} 2>/dev/null; do sleep 0.2; done; open \"$0\"", std::process::id());
        std::process::Command::new("/bin/sh").arg("-c").arg(script).arg(path).spawn()
    } else {
        std::process::Command::new(path).spawn()
    };
    if let Err(e) = spawned {
        log_warn!("update: relaunch failed: {e}");
    }
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("{program}: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

fn path_str(p: &Path) -> Result<&str, String> {
    p.to_str().ok_or_else(|| format!("unsupported path {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.10.0", "0.9.3"));
        assert!(is_newer("v1.0.0", "0.99.0"));
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn checksum_lines() {
        let sums = "abc123  haze-0.3.0-macos.dmg\nDEF456  haze-0.3.0-windows-x64.zip\n";
        assert_eq!(expected_sha256(sums, "haze-0.3.0-windows-x64.zip").as_deref(), Some("def456"));
        assert_eq!(expected_sha256(sums, "haze-0.3.0-macos.dmg").as_deref(), Some("abc123"));
        assert_eq!(expected_sha256(sums, "haze-0.3.0-linux-x86_64"), None);
    }

    #[test]
    fn package_per_platform() {
        let linux = format!("haze-0.3.0-linux-{}", std::env::consts::ARCH);
        let release = Release {
            version: "0.3.0".into(),
            page: String::new(),
            assets: vec![
                ("haze-0.3.0-macos.dmg".into(), "u1".into()),
                ("haze-0.3.0-windows-x64.zip".into(), "u2".into()),
                (linux.clone(), "u3".into()),
            ],
        };
        let expected = if cfg!(target_os = "macos") {
            Some(("haze-0.3.0-macos.dmg".to_string(), "u1"))
        } else if cfg!(windows) {
            Some(("haze-0.3.0-windows-x64.zip".to_string(), "u2"))
        } else {
            Some((linux, "u3"))
        };
        assert_eq!(release.package(), expected);
    }

    #[test]
    fn a_folder_swaps_its_files_and_keeps_the_rest() {
        let root = std::env::temp_dir().join(format!("haze-swap-test-{}", std::process::id()));
        let (dir, staged) = (root.join("app"), root.join("staged"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&staged).unwrap();
        for (name, text) in [("haze.exe", "old"), ("avcodec-61.dll", "old"), ("notes.txt", "mine")] {
            std::fs::write(dir.join(name), text).unwrap();
        }
        for (name, text) in [("haze.exe", "new"), ("avcodec-62.dll", "new")] {
            std::fs::write(staged.join(name), text).unwrap();
        }
        swap_folder(&staged, &dir).unwrap();
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
        assert_eq!(read("haze.exe").as_deref(), Some("new"));
        assert_eq!(read("avcodec-62.dll").as_deref(), Some("new"));
        assert_eq!(read("notes.txt").as_deref(), Some("mine"));
        assert_eq!(read("haze.exe.old"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
