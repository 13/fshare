use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

pub fn read_dir_entries(dir: &Path, show_hidden: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(Entry { name, is_dir: md.is_dir(), size: md.len(), mtime });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    out
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

fn enc(seg: &str) -> String {
    percent_encoding::utf8_percent_encode(seg, percent_encoding::NON_ALPHANUMERIC).to_string()
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
            r#"<tr><td class="n"><a href="{dir_url}/..">../</a></td><td></td><td></td></tr>"#
        ));
    }
    for e in entries {
        let name_enc = enc(&e.name);
        let name_disp = html_escape::encode_text(&e.name);
        let (href, icon, size, sort_size) = if e.is_dir {
            (format!("{dir_url}/{name_enc}/"), "📁", String::new(), 0)
        } else {
            (format!("{dir_url}/{name_enc}"), "📄", human_size(e.size), e.size)
        };
        let date = chrono::DateTime::from_timestamp(e.mtime, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        rows.push_str(&format!(
            r#"<tr><td class="n" data-s="{n}"><a href="{href}">{icon} {name_disp}{slash}</a></td><td class="s" data-s="{sort_size}">{size}</td><td class="d" data-s="{mt}">{date}</td></tr>"#,
            n = name_disp,
            slash = if e.is_dir { "/" } else { "" },
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
        let e = read_dir_entries(t.path(), false);
        let names: Vec<_> = e.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["zdir", "a.txt", "b.txt"]);
        assert!(read_dir_entries(t.path(), true).iter().any(|e| e.name == ".hid"));
    }

    #[test]
    fn html_contains_links_breadcrumbs_zip() {
        let entries = vec![
            Entry { name: "sub dir".into(), is_dir: true, size: 0, mtime: 0 },
            Entry { name: "a<b.txt".into(), is_dir: false, size: 5, mtime: 0 },
        ];
        let html = render_html("docs/x", &entries, "", true, false);
        assert!(html.contains("sub%20dir/")); // percent-encoded href
        assert!(html.contains("a&lt;b.txt")); // escaped display name
        assert!(html.contains("?zip")); // zip button
        assert!(html.contains("docs")); // breadcrumb
        let noz = render_html("", &entries, "", false, false);
        assert!(!noz.contains("?zip"));
    }

    #[test]
    fn upload_ui_gated() {
        let html = render_html("", &[], "", false, true);
        assert!(html.contains("dropzone") && html.contains("XMLHttpRequest"));
        let none = render_html("", &[], "", false, false);
        assert!(!none.contains("dropzone"));
    }
}
