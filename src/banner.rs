pub fn fits(term_cols: usize, qr_w: usize, addr_w: usize) -> bool {
    term_cols >= qr_w + 2 + addr_w
}

/// `qr` lines are pre-indented and rectangular; `addr` is (plain, colored).
/// Widths are computed from plain text; colored text is what gets printed.
pub fn side_by_side(qr: &[String], addr: &[(String, String)]) -> Vec<String> {
    let qr_w = qr.first().map(|l| l.chars().count()).unwrap_or(0);
    // quiet-zone rows above the QR are blank; start the address column at
    // the first visible row so the QR top and first address share a line
    let offset = qr.iter().take_while(|l| l.trim().is_empty()).count();
    let height = qr.len().max(addr.len() + offset);
    (0..height)
        .map(|i| {
            let left = qr.get(i).map(String::as_str).unwrap_or("");
            let pad = qr_w.saturating_sub(left.chars().count());
            let right = i
                .checked_sub(offset)
                .and_then(|j| addr.get(j))
                .map(|(_, c)| c.as_str())
                .unwrap_or("");
            format!("{left}{}  {right}", " ".repeat(pad)).trim_end().to_string()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_boundary() {
        assert!(fits(80, 40, 38)); // 40+2+38 == 80
        assert!(!fits(79, 40, 38));
    }

    fn qr3() -> Vec<String> {
        vec!["  ███".into(), "  █ █".into(), "  ███".into()]
    }

    #[test]
    fn zips_addr_shorter() {
        let addr = vec![("a".into(), "A".into())];
        let out = side_by_side(&qr3(), &addr);
        assert_eq!(out, vec!["  ███  A", "  █ █", "  ███"]);
    }

    #[test]
    fn zips_addr_longer() {
        let addr: Vec<_> = (0..5).map(|i| (format!("p{i}"), format!("C{i}"))).collect();
        let out = side_by_side(&qr3(), &addr);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], "  ███  C0");
        assert_eq!(out[3], "       C3");
        assert_eq!(out[4], "       C4");
    }

    #[test]
    fn empty_qr_degenerates() {
        let addr = vec![("p".into(), "C".into())];
        assert_eq!(side_by_side(&[], &addr), vec!["  C"]);
    }

    #[test]
    fn addr_aligns_with_first_visible_qr_row() {
        // quiet zone: two blank rows above the QR proper
        let qr = vec!["     ".into(), "     ".into(), "  ███".into(), "  █ █".into()];
        let addr = vec![("a".into(), "A".into()), ("b".into(), "B".into())];
        let out = side_by_side(&qr, &addr);
        // first address sits beside the first VISIBLE row, not the quiet zone
        assert_eq!(out, vec!["", "", "  ███  A", "  █ █  B"]);
    }
}
