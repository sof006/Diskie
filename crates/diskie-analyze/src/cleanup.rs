//! Well-known space users, with what each one is and how to deal with it.
//!
//! Matching is by name and surroundings only (a `target` folder next to a `Cargo.toml`), from
//! the scanned tree, without touching the disk. Once a folder matches, nothing inside it is
//! looked at, so suggestions never overlap.

use diskie_model::Tree;
use diskie_model::tree::flag;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Safety {
    /// Rebuilt or downloaded again when needed.
    Safe,
    /// Usually fine to delete, but look first.
    Review,
    /// Don't delete it by hand: the advice says which tool to use.
    UseTool,
}

impl Safety {
    pub fn label(self) -> &'static str {
        match self {
            Safety::Safe => "Safe to delete",
            Safety::Review => "Check first",
            Safety::UseTool => "Use the system tool",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Suggestion {
    pub node: u32,
    pub what: &'static str,
    pub advice: &'static str,
    pub safety: Safety,
}

/// Suggestions at or below `under` for items of at least `min_size` bytes, largest first.
pub fn suggest(tree: &Tree, under: u32, min_size: u64, now: u64) -> Vec<Suggestion> {
    let mut out = Vec::new();
    let mut stack = vec![under];
    while let Some(n) = stack.pop() {
        if tree.size[n as usize] < min_size {
            continue;
        }
        if n != under
            && let Some((what, advice, safety)) = rule(tree, n, now)
        {
            out.push(Suggestion { node: n, what, advice, safety });
            continue;
        }
        if tree.is_dir(n) && !tree.has(n, flag::REPARSE) {
            stack.extend_from_slice(tree.kids(n));
        }
    }
    out.sort_by(|a, b| tree.size[b.node as usize].cmp(&tree.size[a.node as usize]));
    out
}

type Rule = (&'static str, &'static str, Safety);

fn rule(t: &Tree, n: u32, now: u64) -> Option<Rule> {
    let name = t.name(n).to_ascii_lowercase();
    let parent = t.parent[n as usize];
    let parent_name = if parent == diskie_model::NONE { String::new() } else { t.name(parent).to_ascii_lowercase() };
    let sibling = |s: &str| parent != diskie_model::NONE && t.kids(parent).iter().any(|&k| t.name(k).eq_ignore_ascii_case(s));
    let sibling_ext = |e: &str| parent != diskie_model::NONE && t.kids(parent).iter().any(|&k| t.name(k).to_ascii_lowercase().ends_with(e));
    let child = |s: &str| t.kids(n).iter().any(|&k| t.name(k).eq_ignore_ascii_case(s));
    let path = || t.path(n).to_ascii_lowercase().replace('\\', "/");
    use Safety::*;

    if !t.is_dir(n) {
        return match name.as_str() {
            "hiberfil.sys" => Some((
                "Hibernation file",
                "Holds memory while the PC hibernates. Turning hibernation off with `powercfg /h off` (as administrator) deletes it, but also turns off Fast Startup.",
                UseTool,
            )),
            "pagefile.sys" | "swapfile.sys" => Some((
                "Virtual memory",
                "Windows' page file. Change its size in System Properties › Advanced › Performance › Virtual memory; never delete it by hand.",
                UseTool,
            )),
            "memory.dmp" => Some(("Crash dump", "Written when Windows crashed. Only needed to investigate that crash.", Safe)),
            "ext4.vhdx" | "docker_data.vhdx" | "docker.raw" => Some((
                "WSL or Docker disk",
                "A Linux disk that grows but never shrinks on its own. Delete files inside it, run `wsl --shutdown`, then compact it (`Optimize-VHD`, or diskpart's `compact vdisk`).",
                UseTool,
            )),
            _ if name.ends_with(".dmp") && (parent_name == "crashdumps" || parent_name == "minidump") => {
                Some(("Crash dump", "Written when a program crashed. Only needed to investigate that crash.", Safe))
            }
            _ if parent_name == "downloads"
                && [".exe", ".msi", ".iso", ".zip", ".7z", ".rar", ".dmg", ".appimage", ".deb", ".rpm", ".tar.gz", ".tar.xz"].iter().any(|e| name.ends_with(e))
                && now.saturating_sub(t.mtime[n as usize] as u64) > 30 * 86_400 =>
            {
                Some(("Old download", "An installer or archive downloaded over a month ago. Usually safe once installed or unpacked.", Review))
            }
            _ => None,
        };
    }

    let build = Some(("Build output", "Created by building the project next to it; building again recreates it.", Safe));
    let pkg_cache = Some(("Package download cache", "Packages a package manager downloaded. Anything needed is downloaded again.", Safe));
    let app_cache = Some(("App cache", "Data an app keeps to start or load faster; the app rebuilds it.", Safe));
    let shaders = Some(("Shader cache", "Compiled graphics shaders; rebuilt the next time games or apps start.", Safe));
    match name.as_str() {
        "node_modules" => Some(("JavaScript packages", "Installed by npm, yarn or pnpm; `npm install` puts them back.", Safe)),
        "target" if sibling("Cargo.toml") => build,
        "build" if sibling("build.gradle") || sibling("build.gradle.kts") || sibling("CMakeLists.txt") || sibling("meson.build") => build,
        "obj" | "bin" if sibling_ext(".csproj") || sibling_ext(".vbproj") || sibling_ext(".fsproj") => build,
        ".next" | ".nuxt" | ".parcel-cache" | ".turbo" | ".angular" | ".svelte-kit" if sibling("package.json") => build,
        "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | ".tox" => {
            Some(("Python cache", "Compiled Python files and tool caches; Python recreates them.", Safe))
        }
        ".venv" | "venv" | "env" | ".env" if child("pyvenv.cfg") => Some((
            "Python virtual environment",
            "Packages installed for one project. Recreate it with `python -m venv` and `pip install -r requirements.txt`.",
            Review,
        )),
        ".godot" if sibling("project.godot") => Some(("Godot import cache", "Imported assets; Godot reimports them when the project opens.", Safe)),
        "library" if sibling("Assets") && sibling("ProjectSettings") => Some(("Unity library", "Unity's import cache; it is rebuilt when the project opens (which can take a while).", Safe)),
        "intermediate" | "deriveddatacache" if sibling_ext(".uproject") => Some(("Unreal build cache", "Unreal rebuilds it; the first open afterwards is slower.", Safe)),
        "deriveddata" if parent_name == "xcode" => build,
        ".gradle" | "caches" if parent_name == ".gradle" || name == ".gradle" => Some(("Gradle cache", "Downloaded dependencies and build caches; Gradle downloads them again.", Safe)),
        "registry" if parent_name == ".cargo" => pkg_cache,
        "_cacache" | "_npx" if parent_name == ".npm" || parent_name == "npm-cache" => pkg_cache,
        ".pnpm-store" | ".yarn-cache" => pkg_cache,
        "cache" if parent_name == ".yarn" || parent_name == "pip" || parent_name == "yarn" || parent_name == "go-build" => pkg_cache,
        "repository" if parent_name == ".m2" => pkg_cache,
        "packages" if parent_name == ".nuget" => pkg_cache,
        "mod" if parent_name == "pkg" && path().contains("/go/pkg/mod") => pkg_cache,
        "temp" | "tmp" if parent_name == "local" || parent_name == "windows" => Some((
            "Temporary files",
            "Left behind by installers and apps. Close programs first; files still in use can't be deleted and are skipped.",
            Safe,
        )),
        "windows.old" => Some((
            "Previous Windows installation",
            "Kept after a Windows upgrade so you can go back. Remove it with Settings › System › Storage › Temporary files › Previous Windows installation(s).",
            UseTool,
        )),
        "winsxs" if parent_name == "windows" => Some((
            "Windows component store",
            "Never delete it by hand. Shrink it with `Dism /Online /Cleanup-Image /StartComponentCleanup` as administrator.",
            UseTool,
        )),
        "download" if parent_name == "softwaredistribution" => Some((
            "Windows Update downloads",
            "Updates already installed or waiting. Use Disk Cleanup › Windows Update Cleanup, or stop the Windows Update service before deleting.",
            UseTool,
        )),
        "$recycle.bin" => Some(("Recycle Bin", "Things you deleted. Empty it from the drive list on the left.", UseTool)),
        "trash" if parent_name == "share" && path().contains("/.local/share/trash") => {
            Some(("Trash", "Things you deleted. Empty it from the drive list on the left.", UseTool))
        }
        "package cache" if parent_name == "programdata" => Some((
            "Installer cache",
            "Copies of installers that programs need to repair or uninstall themselves. Don't delete it.",
            UseTool,
        )),
        "crashdumps" | "minidump" | "livekernelreports" => Some(("Crash dumps", "Written when programs or Windows crashed. Only needed to investigate those crashes.", Safe)),
        "d3dscache" | "dxcache" | "glcache" | "shadercache" | "grshadercache" | "nv_cache" => shaders,
        "cache" | "cache_data" | "code cache" | "gpucache" | "cache2" if path().contains("/appdata/") || path().contains("/.cache/") || path().contains("/.mozilla/") => app_cache,
        ".cache" if path().starts_with("/home/") || path().starts_with("/root/") => Some((
            "Cache folder",
            "Where Linux apps keep caches and thumbnails. Safe to empty, though some apps start slower once.",
            Review,
        )),
        "archives" if path().ends_with("/var/cache/apt/archives") => Some(("APT package cache", "Downloaded .deb packages. `sudo apt clean` empties it.", UseTool)),
        "journal" if path().ends_with("/var/log/journal") => Some(("System journal", "System logs. `sudo journalctl --vacuum-size=200M` trims them.", UseTool)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::tree;

    #[test]
    fn finds_build_output_downloads_and_system_files() {
        let t = tree();
        let s = suggest(&t, Tree::ROOT, 1, 2_000_000_100);
        let found: Vec<(&str, &str)> = s.iter().map(|x| (t.name(x.node), x.what)).collect();
        assert_eq!(found, [("pagefile.sys", "Virtual memory"), ("setup.exe", "Old download"), ("target", "Build output")]);
        assert_eq!(s[0].safety, Safety::UseTool);
        assert!(suggest(&t, Tree::ROOT, 10_000_000, 2_000_000_100).iter().all(|x| t.name(x.node) != "target"), "min size applies");
    }
}
