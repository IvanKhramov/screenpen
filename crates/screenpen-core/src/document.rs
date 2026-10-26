use std::collections::HashSet;
use std::sync::Arc;

use crate::annotation::{Annotation, AnnotationId, Geometry};

/// How many steps undo keeps.
const UNDO_LIMIT: usize = 200;

/// The annotations on one screen. Vector order is z-order (last is on top).
///
/// Every mutation saves the previous vector, so undo and redo always restore an
/// exact earlier state. Annotations are shared through `Arc`, so a snapshot only
/// copies pointers.
#[derive(Clone, Debug, Default)]
pub struct Document {
    annotations: Vec<Arc<Annotation>>,
    undo: Vec<Vec<Arc<Annotation>>>,
    redo: Vec<Vec<Arc<Annotation>>>,
}

impl Document {
    pub fn new() -> Document {
        Document::default()
    }

    pub fn annotations(&self) -> &[Arc<Annotation>] {
        &self.annotations
    }

    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        self.annotations
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.as_ref())
    }

    pub fn insert(&mut self, annotation: Annotation) {
        let mut next = self.annotations.clone();
        next.push(Arc::new(annotation));
        self.replace(next);
    }

    /// Adds several annotations as one undoable step (restoring a snapshot).
    pub fn insert_all(&mut self, annotations: Vec<Annotation>) {
        if annotations.is_empty() {
            return;
        }
        let mut next = self.annotations.clone();
        next.extend(annotations.into_iter().map(Arc::new));
        self.replace(next);
    }

    pub fn remove(&mut self, ids: &HashSet<AnnotationId>) {
        let remaining: Vec<_> = self
            .annotations
            .iter()
            .filter(|a| !ids.contains(&a.id))
            .cloned()
            .collect();
        if remaining.len() != self.annotations.len() {
            self.replace(remaining);
        }
    }

    /// Applies `change` to each annotation in `ids` as one undoable step.
    pub fn update(&mut self, ids: &HashSet<AnnotationId>, mut change: impl FnMut(&mut Annotation)) {
        if ids.is_empty() {
            return;
        }
        let mut changed = false;
        let next: Vec<_> = self
            .annotations
            .iter()
            .map(|a| {
                if !ids.contains(&a.id) {
                    return a.clone();
                }
                let mut copy = Annotation::clone(a);
                change(&mut copy);
                if copy == **a {
                    a.clone()
                } else {
                    changed = true;
                    Arc::new(copy)
                }
            })
            .collect();
        if changed {
            self.replace(next);
        }
    }

    /// Moves `ids` to the top of the z-order, keeping their relative order.
    pub fn bring_to_front(&mut self, ids: &HashSet<AnnotationId>) {
        let (front, rest): (Vec<_>, Vec<_>) = self
            .annotations
            .iter()
            .cloned()
            .partition(|a| ids.contains(&a.id));
        let next: Vec<_> = rest.into_iter().chain(front).collect();
        if next
            .iter()
            .map(|a| a.id)
            .ne(self.annotations.iter().map(|a| a.id))
        {
            self.replace(next);
        }
    }

    /// One more than the highest badge number on this screen, so undo naturally
    /// "gives back" a number.
    pub fn next_badge_number(&self) -> u32 {
        self.annotations
            .iter()
            .filter_map(|a| match a.geometry {
                Geometry::Badge { number, .. } => Some(number),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            + 1
    }

    pub fn remove_all(&mut self) {
        if !self.annotations.is_empty() {
            self.replace(Vec::new());
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Returns false when there was nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push(std::mem::replace(&mut self.annotations, previous));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo
            .push(std::mem::replace(&mut self.annotations, next));
        true
    }

    /// Removes annotations without an undo step (auto-fade, not a user action).
    pub fn expire(&mut self, ids: &HashSet<AnnotationId>) {
        self.annotations.retain(|a| !ids.contains(&a.id));
    }

    fn replace(&mut self, next: Vec<Arc<Annotation>>) {
        self.undo
            .push(std::mem::replace(&mut self.annotations, next));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
}
