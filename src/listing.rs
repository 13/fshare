use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

pub fn read_dir_entries(dir: &Path, show_hidden: bool, dir_sizes: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let size = if md.is_dir() {
            if dir_sizes { dir_size(&e.path()) } else { 0 }
        } else {
            md.len()
        };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(Entry { name, is_dir: md.is_dir(), size, mtime });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    out
}

fn dir_size(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn icon_for(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "📁";
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "avif" | "heic" => {
            "🖼️"
        }
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" => "🎬",
        "mp3" | "flac" | "ogg" | "wav" | "m4a" | "opus" | "aac" => "🎵",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" => "📦",
        "pdf" | "epub" | "mobi" => "📕",
        "rs" | "py" | "js" | "ts" | "jsx" | "tsx" | "c" | "h" | "cpp" | "hpp" | "go" | "java"
        | "kt" | "rb" | "php" | "sh" | "zsh" | "lua" | "sql" | "html" | "css" | "json"
        | "yaml" | "yml" | "toml" | "xml" => "💻",
        "md" | "txt" | "rst" | "org" | "log" => "📝",
        "bin" | "so" | "exe" | "dll" | "deb" | "rpm" | "appimage" | "iso" | "img" => "⚙️",
        "pem" | "key" | "crt" | "pub" | "asc" | "gpg" => "🔑",
        _ => "📄",
    }
}

/// Encode a single path segment for use in an href. Only characters that
/// break URLs or HTML attributes are escaped; `-`, `_`, `.`, `~` stay raw.
const PATH_SEG: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'\'')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'?')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'^')
    .add(b'{')
    .add(b'}')
    .add(b'|')
    .add(b'\\');

fn enc(seg: &str) -> String {
    percent_encoding::utf8_percent_encode(seg, PATH_SEG).to_string()
}

const UPLOAD_BLOCK: &str = r#"<div id="dropzone" style="border:2px dashed var(--line);border-radius:8px;padding:1em;text-align:center;color:var(--muted);margin-bottom:1rem;cursor:pointer">
  drop files here or click to upload
  <input type="file" id="fpick" multiple style="display:none">
  <div id="uplist"></div>
</div>
<script>
(() => {
  const dz = document.getElementById('dropzone');
  const fp = document.getElementById('fpick');
  const list = document.getElementById('uplist');
  dz.onclick = () => fp.click();
  fp.onchange = () => sendAll(fp.files);
  ['dragover','dragenter'].forEach(ev => document.addEventListener(ev, e => {
    e.preventDefault(); dz.style.borderColor = 'var(--accent)';
  }));
  ['dragleave','drop'].forEach(ev => document.addEventListener(ev, e => {
    e.preventDefault(); dz.style.borderColor = 'var(--line)';
  }));
  document.addEventListener('drop', e => sendAll(e.dataTransfer.files));
  function sendAll(files) {
    let pending = files.length;
    [...files].forEach(f => {
      const row = document.createElement('div');
      row.textContent = f.name + ' 0%';
      list.appendChild(row);
      const fd = new FormData();
      fd.append('file', f);
      const xhr = new XMLHttpRequest();
      xhr.open('POST', location.pathname);
      xhr.setRequestHeader('Accept', 'application/json');
      xhr.upload.onprogress = e => {
        if (e.lengthComputable) row.textContent = f.name + ' ' + Math.round(100*e.loaded/e.total) + '%';
      };
      xhr.onload = () => {
        row.textContent = f.name + (xhr.status < 300 ? ' ✓' : ' ✗ ' + xhr.responseText);
        if (--pending === 0 && xhr.status < 300) location.reload();
      };
      xhr.onerror = () => { row.textContent = f.name + ' ✗ network error'; --pending; };
      xhr.send(fd);
    });
  }
})();
</script>"#;

pub fn render_html(
    rel_path: &str,
    entries: &[Entry],
    base: &str,
    zip: bool,
    upload: bool,
    dir_sizes: bool,
) -> String {
    let template = include_str!("listing.html");

    // breadcrumbs: root link + each path segment
    let mut crumbs = format!(r#"<a href="{base}/">fshare</a>"#);
    let mut acc = String::new();
    for seg in rel_path.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(&enc(seg));
        crumbs.push_str(&format!(
            r#" / <a href="{base}{acc}/">{}</a>"#,
            html_escape::encode_text(seg)
        ));
    }

    let dir_url = {
        let mut u = String::from(base);
        for seg in rel_path.split('/').filter(|s| !s.is_empty()) {
            u.push('/');
            u.push_str(&enc(seg));
        }
        u
    };

    let mut rows = String::new();
    if !rel_path.is_empty() {
        rows.push_str(&format!(
            r#"<tr class="up"><td class="n"><a href="{dir_url}/..">../</a></td><td></td><td></td></tr>"#
        ));
    }
    for e in entries {
        let name_enc = enc(&e.name);
        let name_disp = html_escape::encode_text(&e.name);
        let icon = icon_for(&e.name, e.is_dir);
        let slash = if e.is_dir { "/" } else { "" };
        let href = format!("{dir_url}/{name_enc}{slash}");
        let size =
            if e.is_dir && !dir_sizes { String::new() } else { human_size(e.size) };
        let sort_size = e.size;
        let date = chrono::DateTime::from_timestamp(e.mtime, 0)
            .map(|d| d.format(r#"%Y-%m-%d <span class="tm">%H:%M</span>"#).to_string())
            .unwrap_or_default();
        let cls = if e.is_dir { r#" class="dir""# } else { "" };
        rows.push_str(&format!(
            r#"<tr{cls}><td class="n" data-s="{n}"><a href="{href}">{icon} {name_disp}{slash}</a></td><td class="s" data-s="{sort_size}">{size}</td><td class="d" data-s="{mt}">{date}</td></tr>"#,
            n = name_disp,
            mt = e.mtime,
        ));
    }

    let zip_btn = if zip {
        format!(r#"<a class="zip" href="{dir_url}/?zip">⬇ Download all (.zip)</a>"#)
    } else {
        String::new()
    };

    template
        .replace(
            "{{title}}",
            &html_escape::encode_text(if rel_path.is_empty() { "/" } else { rel_path }),
        )
        .replace("{{crumbs}}", &crumbs)
        .replace("{{zip}}", &zip_btn)
        .replace("{{upload}}", if upload { UPLOAD_BLOCK } else { "" })
        .replace("{{rows}}", &rows)
        .replace("{{version}}", env!("CARGO_PKG_VERSION"))
        .replace("{{built}}", env!("FSHARE_BUILD_DATE"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(312 * 1024 * 1024), "312 MB");
        assert_eq!(human_size(1395864371), "1.3 GB");
    }

    #[test]
    fn lists_dirs_first_sorted_hidden_filtered() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("b.txt"), "x").unwrap();
        std::fs::write(t.path().join("a.txt"), "x").unwrap();
        std::fs::write(t.path().join(".hid"), "x").unwrap();
        std::fs::create_dir(t.path().join("zdir")).unwrap();
        let e = read_dir_entries(t.path(), false, false);
        let names: Vec<_> = e.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["zdir", "a.txt", "b.txt"]);
        assert!(read_dir_entries(t.path(), true, false).iter().any(|e| e.name == ".hid"));
    }

    #[test]
    fn html_contains_links_breadcrumbs_zip() {
        let entries = vec![
            Entry { name: "sub dir".into(), is_dir: true, size: 0, mtime: 0 },
            Entry { name: "a<b.txt".into(), is_dir: false, size: 5, mtime: 0 },
        ];
        let html = render_html("docs/x", &entries, "", true, false, false);
        assert!(html.contains("sub%20dir/")); // percent-encoded href
        assert!(html.contains("a&lt;b.txt")); // escaped display name
        assert!(html.contains("?zip")); // zip button
        assert!(html.contains("docs")); // breadcrumb
        let noz = render_html("", &entries, "", false, false, false);
        assert!(!noz.contains("?zip"));
    }

    #[test]
    fn date_cells_reformatted_to_local_time_client_side() {
        let entries =
            vec![Entry { name: "a.txt".into(), is_dir: false, size: 5, mtime: 1752000000 }];
        let html = render_html("", &entries, "", false, false, false);
        assert!(html.contains(r#"data-s="1752000000""#)); // epoch for client JS
        assert!(html.contains("td.d[data-s]")); // client-side local-time rewrite
    }

    #[test]
    fn icons_by_extension() {
        assert_eq!(icon_for("x", true), "📁");
        assert_eq!(icon_for("a.PNG", false), "🖼️");
        assert_eq!(icon_for("m.mkv", false), "🎬");
        assert_eq!(icon_for("s.flac", false), "🎵");
        assert_eq!(icon_for("z.tar", false), "📦");
        assert_eq!(icon_for("d.pdf", false), "📕");
        assert_eq!(icon_for("c.rs", false), "💻");
        assert_eq!(icon_for("n.md", false), "📝");
        assert_eq!(icon_for("b.iso", false), "⚙️");
        assert_eq!(icon_for("k.pem", false), "🔑");
        assert_eq!(icon_for("unknown.qqq", false), "📄");
        assert_eq!(icon_for("noext", false), "📄");
    }

    #[test]
    fn enc_keeps_safe_chars() {
        assert_eq!(enc("my-dir_1.x~y"), "my-dir_1.x~y"); // no %2D etc.
        assert_eq!(enc("a b"), "a%20b");
        assert_eq!(enc("a/b"), "a%2Fb");
        assert_eq!(enc("a%b"), "a%25b");
        assert_eq!(enc("a?b#c"), "a%3Fb%23c");
    }

    #[test]
    fn crumbs_show_decoded_names() {
        let html = render_html("my-dir/sub dir", &[], "", false, false, false);
        assert!(html.contains(">my-dir</a>"), "dash must not be encoded in display");
        assert!(html.contains(">sub dir</a>"), "space must display raw");
        assert!(html.contains("my-dir/sub%20dir/"), "href keeps dash, encodes space");
    }

    #[test]
    fn dirs_have_recursive_size() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("d")).unwrap();
        std::fs::write(t.path().join("d/x.bin"), vec![0u8; 3000]).unwrap();
        std::fs::create_dir(t.path().join("d/deep")).unwrap();
        std::fs::write(t.path().join("d/deep/y.bin"), vec![0u8; 2000]).unwrap();
        let e = read_dir_entries(t.path(), false, true);
        let d = e.iter().find(|e| e.name == "d").unwrap();
        assert!(d.is_dir);
        assert_eq!(d.size, 5000);
        let html = render_html("", &e, "", false, false, true);
        assert!(html.contains("4.9 KB"), "dir size shown");
        // default off: size stays 0, cell blank
        let off = read_dir_entries(t.path(), false, false);
        assert_eq!(off.iter().find(|e| e.name == "d").unwrap().size, 0);
        let html_off = render_html("", &off, "", false, false, false);
        assert!(!html_off.contains("4.9 KB"));
    }

    #[test]
    fn mobile_nav_arrows_present() {
        let html = render_html("", &[], "", false, false, false);
        assert!(html.contains("navbtns"));
        assert!(html.contains("history.back()"));
        assert!(html.contains("history.forward()"));
    }

    #[test]
    fn footer_shows_version() {
        let html = render_html("", &[], "", false, false, false);
        assert!(html.contains(env!("CARGO_PKG_VERSION")));
        assert!(html.contains("footer"));
    }

    #[test]
    fn upload_ui_gated() {
        let html = render_html("", &[], "", false, true, false);
        assert!(html.contains("dropzone") && html.contains("XMLHttpRequest"));
        let none = render_html("", &[], "", false, false, false);
        assert!(!none.contains("dropzone"));
    }
}
