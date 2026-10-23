//! Pointer-driven reordering for lists of tunnel cards.
//!
//! The cards are plain divs, so reordering uses mouse events rather than the
//! webview's drag-and-drop, which the desktop event bridge only fills in for
//! files. Pressing a card's header arms a drag; moving the pointer past a few
//! pixels starts it.

use dioxus::prelude::*;

/// How far the pointer moves before a press on a header becomes a drag.
const DRAG_THRESHOLD: f64 = 6.0;

/// Reordering state for one list of cards.
///
/// The list marks its container with `data-reorder-list="{name}"` and each
/// card with `data-reorder-card`. When a drag starts, those cards' vertical
/// extents are read from the page; the pointer is followed with an insertion
/// line and, on release, the dragged ID and the index it should land at are
/// handed back.
#[derive(Clone, Copy, PartialEq)]
pub struct DragReorder {
    /// The `data-reorder-list` name of the list this state belongs to.
    list: &'static str,
    start: Signal<Option<Start>>,
    dragging: Signal<Option<String>>,
    /// Insertion index in the list, from 0 to its length.
    target: Signal<Option<usize>>,
    /// Vertical extents of the cards in display order, from the drag's start.
    extents: Signal<Option<Vec<(f64, f64)>>>,
}

#[derive(Clone)]
struct Start {
    id: String,
    x: f64,
    y: f64,
}

impl DragReorder {
    /// Creates the state for the list whose container has
    /// `data-reorder-list="{list}"`.
    pub fn new(list: &'static str) -> Self {
        Self {
            list,
            start: use_signal(|| None),
            dragging: use_signal(|| None),
            target: use_signal(|| None),
            extents: use_signal(|| None),
        }
    }

    /// Arms a drag from a press on the header of the card with `id`.
    pub fn press(&self, id: &str, x: f64, y: f64) {
        let mut start = self.start;
        start.set(Some(Start {
            id: id.to_string(),
            x,
            y,
        }));
    }

    /// Follows the pointer while the button may be held. Returns true while
    /// a drag is underway, so callers can stop the press from selecting text.
    pub fn pointer_moved(&self, x: f64, y: f64, held: bool) -> bool {
        // The button can be released outside the list, where no event reaches
        // us, so a move without it ends any press or drag still around.
        if !held {
            self.cancel();
            return false;
        }
        let Some(start) = (self.start)() else {
            return false;
        };
        if self.dragging().is_none() {
            let moved =
                (x - start.x).abs() >= DRAG_THRESHOLD || (y - start.y).abs() >= DRAG_THRESHOLD;
            if !moved {
                return false;
            }
            let mut dragging = self.dragging;
            dragging.set(Some(start.id.clone()));
            self.read_extents();
        }
        if let Some(extents) = (self.extents)() {
            // An empty read means the page didn't have the cards marked, so
            // there is no sensible place to drop.
            if extents.is_empty() {
                return true;
            }
            let target = insertion_index(&extents, y);
            if (self.target)() != Some(target) {
                let mut signal = self.target;
                signal.set(Some(target));
            }
            return true;
        }
        true
    }

    /// Ends a drag, handing back the dragged card and the index to drop it
    /// at. Nothing is returned when the press never became a drag.
    pub fn release(&self) -> Option<(String, usize)> {
        let released = self.dragging().zip((self.target)());
        self.cancel();
        released
    }

    /// Forgets any press or drag, leaving the order untouched.
    pub fn cancel(&self) {
        if (self.start)().is_none() && (self.dragging)().is_none() {
            return;
        }
        let mut start = self.start;
        start.set(None);
        let mut dragging = self.dragging;
        dragging.set(None);
        let mut target = self.target;
        target.set(None);
        let mut extents = self.extents;
        extents.set(None);
    }

    /// The ID of the card being dragged, while one is.
    pub fn dragging(&self) -> Option<String> {
        (self.dragging)()
    }

    pub fn is_dragging(&self) -> bool {
        (self.dragging)().is_some()
    }

    /// Whether the insertion line belongs above the card at `index`.
    pub fn drops_before(&self, index: usize) -> bool {
        (self.target)() == Some(index)
    }

    /// Whether the insertion line belongs below the last card of a list of
    /// `len` cards.
    pub fn drops_at_end(&self, len: usize) -> bool {
        (self.target)() == Some(len)
    }

    /// Reads the cards' vertical extents from the page once, when the drag
    /// starts. The list does not scroll while the button is held, so they
    /// stay accurate for the rest of the drag.
    fn read_extents(&self) {
        let mut extents = self.extents;
        let selector = format!("[data-reorder-list=\"{}\"]", self.list);
        spawn(async move {
            let mut eval = document::eval(&format!(
                "const list = document.querySelector({selector:?});
                 dioxus.send(list ? [...list.querySelectorAll('[data-reorder-card]')].map((card) => {{
                     const rect = card.getBoundingClientRect();
                     return [rect.top, rect.bottom];
                 }}) : []);"
            ));
            if let Ok(read) = eval.recv::<Vec<(f64, f64)>>().await {
                extents.set(Some(read));
            }
        });
    }
}

/// The index of the first card whose middle is below `y`, or the number of
/// cards when the pointer is below all of them.
fn insertion_index(extents: &[(f64, f64)], y: f64) -> usize {
    extents
        .iter()
        .position(|(top, bottom)| y < (top + bottom) / 2.0)
        .unwrap_or(extents.len())
}

/// The position a card at `from` must move to for a drop at `insertion`,
/// which counts positions in the list before the card is taken out.
pub fn target_index(from: usize, insertion: usize) -> usize {
    if insertion > from {
        insertion - 1
    } else {
        insertion
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_insertion_index() {
        let cards = [(0.0, 100.0), (120.0, 220.0), (240.0, 340.0)];
        assert_eq!(insertion_index(&cards, 10.0), 0);
        // Below the first card's middle, above the second's.
        assert_eq!(insertion_index(&cards, 80.0), 1);
        assert_eq!(insertion_index(&cards, 200.0), 2);
        assert_eq!(insertion_index(&cards, 400.0), 3);
    }

    #[test]
    fn accounts_for_the_card_being_taken_out() {
        assert_eq!(target_index(0, 0), 0);
        assert_eq!(target_index(0, 3), 2);
        assert_eq!(target_index(2, 0), 0);
        assert_eq!(target_index(1, 1), 1);
    }
}
