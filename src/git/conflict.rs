//! Parsing conflict markers and writing a resolved file back.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
}

#[derive(Clone, Debug)]
pub struct Conflict {
    pub ours_label: String,
    pub theirs_label: String,
    pub ours: Vec<String>,
    pub base: Option<Vec<String>>,
    pub theirs: Vec<String>,
    pub pick: Option<Pick>,
}

#[derive(Clone, Debug)]
pub enum Segment {
    Text(Vec<String>),
    Conflict(Conflict),
}

#[derive(Clone, Debug, Default)]
pub struct ConflictFile {
    pub segments: Vec<Segment>,
    pub trailing_newline: bool,
}

impl ConflictFile {
    pub fn parse(content: &str) -> Self {
        enum St {
            Text,
            Ours,
            Base,
            Theirs,
        }
        let mut segments = Vec::new();
        let mut text = Vec::new();
        let mut cur: Option<Conflict> = None;
        let mut st = St::Text;
        for line in content.lines() {
            match st {
                St::Text if line.starts_with("<<<<<<<") => {
                    if !text.is_empty() {
                        segments.push(Segment::Text(std::mem::take(&mut text)));
                    }
                    cur = Some(Conflict {
                        ours_label: line[7..].trim().to_string(),
                        theirs_label: String::new(),
                        ours: Vec::new(),
                        base: None,
                        theirs: Vec::new(),
                        pick: None,
                    });
                    st = St::Ours;
                }
                St::Text => text.push(line.to_string()),
                St::Ours | St::Base if line.starts_with("=======") && line[7..].trim().is_empty() => {
                    st = St::Theirs
                }
                St::Ours if line.starts_with("|||||||") => {
                    cur.as_mut().unwrap().base = Some(Vec::new());
                    st = St::Base;
                }
                St::Ours => cur.as_mut().unwrap().ours.push(line.to_string()),
                St::Base => {
                    if let Some(b) = cur.as_mut().unwrap().base.as_mut() {
                        b.push(line.to_string())
                    }
                }
                St::Theirs if line.starts_with(">>>>>>>") => {
                    let mut c = cur.take().unwrap();
                    c.theirs_label = line[7..].trim().to_string();
                    segments.push(Segment::Conflict(c));
                    st = St::Text;
                }
                St::Theirs => cur.as_mut().unwrap().theirs.push(line.to_string()),
            }
        }
        // An unterminated conflict is kept as plain text so nothing is lost.
        if let Some(c) = cur {
            text.push(format!("<<<<<<< {}", c.ours_label));
            text.extend(c.ours);
            if let Some(b) = c.base {
                text.push("|||||||".into());
                text.extend(b);
            }
            if matches!(st, St::Theirs) {
                text.push("=======".into());
                text.extend(c.theirs);
            }
        }
        if !text.is_empty() {
            segments.push(Segment::Text(text));
        }
        ConflictFile { segments, trailing_newline: content.ends_with('\n') }
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &Conflict> {
        self.segments.iter().filter_map(|s| match s {
            Segment::Conflict(c) => Some(c),
            _ => None,
        })
    }

    pub fn conflict_mut(&mut self, n: usize) -> Option<&mut Conflict> {
        self.segments
            .iter_mut()
            .filter_map(|s| match s {
                Segment::Conflict(c) => Some(c),
                _ => None,
            })
            .nth(n)
    }

    pub fn unresolved(&self) -> usize {
        self.conflicts().filter(|c| c.pick.is_none()).count()
    }

    pub fn pick_all(&mut self, pick: Pick) {
        for s in &mut self.segments {
            if let Segment::Conflict(c) = s {
                c.pick = Some(pick);
            }
        }
    }

    /// The file with every picked conflict resolved; unpicked ones keep
    /// their markers.
    pub fn render(&self) -> String {
        let mut out: Vec<&str> = Vec::new();
        let (ours_m, theirs_m): (Vec<String>, Vec<String>) = self
            .conflicts()
            .map(|c| (format!("<<<<<<< {}", c.ours_label), format!(">>>>>>> {}", c.theirs_label)))
            .unzip();
        let mut n = 0;
        for s in &self.segments {
            match s {
                Segment::Text(t) => out.extend(t.iter().map(String::as_str)),
                Segment::Conflict(c) => {
                    let ours = c.ours.iter().map(String::as_str);
                    let theirs = c.theirs.iter().map(String::as_str);
                    match c.pick {
                        Some(Pick::Ours) => out.extend(ours),
                        Some(Pick::Theirs) => out.extend(theirs),
                        Some(Pick::OursThenTheirs) => out.extend(ours.chain(theirs)),
                        Some(Pick::TheirsThenOurs) => out.extend(theirs.chain(ours)),
                        None => {
                            out.push(&ours_m[n]);
                            out.extend(ours);
                            if let Some(b) = &c.base {
                                out.push("|||||||");
                                out.extend(b.iter().map(String::as_str));
                            }
                            out.push("=======");
                            out.extend(theirs);
                            out.push(&theirs_m[n]);
                        }
                    }
                    n += 1;
                }
            }
        }
        let mut s = out.join("\n");
        if self.trailing_newline && !s.is_empty() {
            s.push('\n');
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "top\n<<<<<<< HEAD\nmine\n=======\nyours\nyours2\n>>>>>>> feature\nbottom\n";

    #[test]
    fn parse_and_resolve() {
        let mut f = ConflictFile::parse(SRC);
        assert_eq!(f.conflicts().count(), 1);
        let c = f.conflicts().next().unwrap();
        assert_eq!((c.ours_label.as_str(), c.theirs_label.as_str()), ("HEAD", "feature"));
        assert_eq!(f.render(), SRC, "unresolved round-trips");
        f.conflict_mut(0).unwrap().pick = Some(Pick::TheirsThenOurs);
        assert_eq!(f.render(), "top\nyours\nyours2\nmine\nbottom\n");
        assert_eq!(f.unresolved(), 0);
    }

    #[test]
    fn diff3_base_is_dropped_on_resolve() {
        let src = "<<<<<<< ours\na\n||||||| base\no\n=======\nb\n>>>>>>> theirs\n";
        let mut f = ConflictFile::parse(src);
        f.pick_all(Pick::Ours);
        assert_eq!(f.render(), "a\n");
    }
}
