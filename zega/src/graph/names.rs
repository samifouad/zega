//! Names stored once per graph (zegadb/zega#100).
//!
//! A label, a relationship kind or a property key is a [`Sym`], a `u32`
//! into one table, instead of a `String` copied into every node. The set of
//! labels and property keys a node carries is a [`ShapeId`] into a second
//! table: nodes of one type nearly always share one shape, so a node stores
//! four bytes for all its names and a slice of values in the shape's key
//! order.
//!
//! Neither table forgets a name or a shape once seen. Both grow with the
//! schema (the distinct labels, keys and key sets ever written), not with
//! the data, and a graph restarted from its snapshot starts them afresh.

use std::collections::HashMap;

/// An interned label, relationship kind or property key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Sym(u32);

#[derive(Clone, Default)]
pub(crate) struct Names {
    names: Vec<Box<str>>,
    ids: HashMap<Box<str>, Sym>,
}

impl Names {
    pub fn intern(&mut self, name: &str) -> Sym {
        if let Some(&sym) = self.ids.get(name) {
            return sym;
        }
        let sym = Sym(u32::try_from(self.names.len()).expect("fewer than 2^32 distinct names"));
        self.names.push(name.into());
        self.ids.insert(name.into(), sym);
        sym
    }

    pub fn get(&self, name: &str) -> Option<Sym> {
        self.ids.get(name).copied()
    }

    pub fn name(&self, sym: Sym) -> &str {
        &self.names[sym.0 as usize]
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.names.len()
    }
}

/// The labels (in the order written) and the property keys (ascending by
/// [`Sym`]) of a node, or the property keys of a relationship.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Shape {
    pub labels: Box<[Sym]>,
    pub keys: Box<[Sym]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShapeId(u32);

#[derive(Clone, Default)]
pub(crate) struct Shapes {
    list: Vec<Shape>,
    ids: HashMap<Shape, ShapeId>,
}

impl Shapes {
    pub fn intern(&mut self, shape: Shape) -> ShapeId {
        if let Some(&id) = self.ids.get(&shape) {
            return id;
        }
        let id = ShapeId(u32::try_from(self.list.len()).expect("fewer than 2^32 distinct shapes"));
        self.list.push(shape.clone());
        self.ids.insert(shape, id);
        id
    }

    pub fn get(&self, id: ShapeId) -> &Shape {
        &self.list[id.0 as usize]
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_stored_once_and_keeps_its_sym() {
        let mut names = Names::default();
        let a = names.intern("name");
        let b = names.intern("age");
        assert_eq!(names.intern("name"), a);
        assert_ne!(a, b);
        assert_eq!(names.name(a), "name");
        assert_eq!(names.get("age"), Some(b));
        assert_eq!(names.get("missing"), None);
        assert_eq!(names.names.len(), 2);
    }

    #[test]
    fn equal_shapes_share_an_id() {
        let mut names = Names::default();
        let mut shapes = Shapes::default();
        let person = names.intern("Person");
        let (x, y) = (names.intern("x"), names.intern("y"));
        let first = shapes.intern(Shape { labels: [person].into(), keys: [x, y].into() });
        let again = shapes.intern(Shape { labels: [person].into(), keys: [x, y].into() });
        let other = shapes.intern(Shape { labels: [person].into(), keys: [x].into() });
        assert_eq!(first, again);
        assert_ne!(first, other);
        assert_eq!(&*shapes.get(other).keys, &[x]);
    }
}
