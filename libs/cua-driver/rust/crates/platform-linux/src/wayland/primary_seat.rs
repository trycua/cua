//! Keep experimental delivery seats out of the existing foreground input path.
//! Preserve the legacy last-advertised-seat choice among ordinary seats.

pub(super) struct Seats<T> {
    entries: Vec<(T, Option<String>)>,
}

impl<T> Default for Seats<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<T: Clone + PartialEq> Seats<T> {
    pub(super) fn add(&mut self, seat: T) {
        self.entries.push((seat, None));
    }

    pub(super) fn name(&mut self, seat: &T, name: String) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| &entry.0 == seat) {
            entry.1 = Some(name);
        }
    }

    pub(super) fn selected(&self) -> Option<T> {
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                !entry.1.as_deref().is_some_and(|name| {
                    name == "Cua-Test-Agent" || name.starts_with("Cua-Test-Agent-")
                })
            })
            .map(|entry| entry.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::Seats;

    #[test]
    fn experimental_seat_never_replaces_primary_after_names_arrive() {
        for order in [[1, 2], [2, 1]] {
            let mut seats = Seats::default();
            for seat in order {
                seats.add(seat);
            }
            seats.name(&1, "seat0".into());
            seats.name(&2, "Cua-Test-Agent".into());
            assert_eq!(seats.selected(), Some(1));
        }
    }

    #[test]
    fn no_primary_seat_refuses_and_legacy_selection_is_preserved() {
        let mut seats = Seats::default();
        seats.add(2);
        seats.name(&2, "Cua-Test-Agent".into());
        assert_eq!(seats.selected(), None);
        seats.add(4);
        seats.name(&4, "Cua-Test-Agent-2".into());
        assert_eq!(seats.selected(), None);
        seats.add(1);
        seats.add(3);
        assert_eq!(seats.selected(), Some(3));
    }
}
