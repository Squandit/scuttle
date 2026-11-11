//! The emoji picker's data: every emoji with its English name.

/// One emoji and what it is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emoji {
    pub character: &'static str,
    pub name: &'static str,
}

/// The emoji table, built into the program. See `data/README.md` for
/// where it comes from.
const TABLE: &str = include_str!("../data/emoji.tsv");

/// Every emoji, in Unicode's own order: smileys first, flags last.
pub fn all() -> impl Iterator<Item = Emoji> {
    TABLE.lines().filter_map(|line| {
        let (character, name) = line.split_once('\t')?;
        Some(Emoji { character, name })
    })
}

/// A name with its first letter capitalised, for display.
pub fn display_name(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_complete_and_well_formed() {
        let emoji: Vec<Emoji> = all().collect();
        assert!(emoji.len() > 1800, "only {} emoji", emoji.len());
        assert_eq!(emoji.len(), TABLE.lines().count(), "every line must parse");
        for e in &emoji {
            assert!(!e.character.is_empty() && !e.name.is_empty());
            assert!(!e.character.is_ascii(), "{} is not an emoji", e.character);
            assert!(!e.name.contains('\t'));
        }
    }

    #[test]
    fn it_starts_with_smileys_and_has_the_usual_suspects() {
        let first = all().next().unwrap();
        assert_eq!((first.character, first.name), ("😀", "grinning face"));
        for wanted in ["red heart", "thumbs up", "fire", "crab", "party popper"] {
            assert!(all().any(|e| e.name == wanted), "missing {wanted}");
        }
    }

    #[test]
    fn skin_tone_variants_are_left_out() {
        assert!(!all().any(|e| e.name.contains("skin tone")));
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<&str> = all().map(|e| e.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total);
    }

    #[test]
    fn display_names_are_capitalised() {
        assert_eq!(display_name("grinning face"), "Grinning face");
        assert_eq!(display_name(""), "");
    }
}
