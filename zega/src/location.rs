//! WGS84 points and a fixed 16-bit-per-axis Morton (Z-order) index.
use crate::graph::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Mean Earth radius, in metres. All distances use the same spherical model.
pub const EARTH_RADIUS: f64 = 6_371_008.8;

/// Validated WGS84 degrees, stored as bits for stable equality and WAL encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point {
    lat: u64,
    lon: u64,
}

impl Point {
    pub fn new(lat: f64, lon: f64) -> Result<Self, &'static str> {
        if !lat.is_finite() || !(-90.0..=90.0).contains(&lat) {
            return Err("Point latitude must be a number in [-90, 90]");
        }
        if !lon.is_finite() || !(-180.0..=180.0).contains(&lon) {
            return Err("Point longitude must be a number in [-180, 180]");
        }
        Ok(Self {
            lat: (if lat == 0.0 { 0.0 } else { lat }).to_bits(),
            lon: (if lon == 0.0 { 0.0 } else { lon }).to_bits(),
        })
    }
    pub fn lat(self) -> f64 {
        f64::from_bits(self.lat)
    }
    pub fn lon(self) -> f64 {
        f64::from_bits(self.lon)
    }
    pub fn from_json(value: &serde_json::Value) -> Result<Self, &'static str> {
        let object = value
            .as_object()
            .filter(|object| object.len() == 2)
            .ok_or("Point needs an object with numeric lat and lon")?;
        let lat = object
            .get("lat")
            .and_then(serde_json::Value::as_f64)
            .ok_or("Point latitude must be a number in [-90, 90]")?;
        let lon = object
            .get("lon")
            .and_then(serde_json::Value::as_f64)
            .ok_or("Point longitude must be a number in [-180, 180]")?;
        Self::new(lat, lon)
    }
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({"lat": self.lat(), "lon": self.lon()})
    }
    pub fn distance(self, other: Self) -> f64 {
        let a = self.lat().to_radians();
        let b = other.lat().to_radians();
        let longitude = ((other.lon() - self.lon() + 180.0).rem_euclid(360.0) - 180.0).to_radians();
        let cos = |lat: f64| {
            if lat.abs() == 90.0 {
                0.0
            } else {
                lat.to_radians().cos()
            }
        };
        let h = ((b - a) / 2.0).sin().powi(2)
            + cos(self.lat()) * cos(other.lat()) * (longitude / 2.0).sin().powi(2);
        2.0 * EARTH_RADIUS * h.clamp(0.0, 1.0).sqrt().asin()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub south: f64,
    pub west: f64,
    pub north: f64,
    pub east: f64,
}
impl Bounds {
    pub fn new(southwest: Point, northeast: Point) -> Result<Self, &'static str> {
        if southwest.lat() > northeast.lat() {
            return Err("box south latitude must not exceed north latitude");
        }
        Ok(Self {
            south: southwest.lat(),
            west: southwest.lon(),
            north: northeast.lat(),
            east: northeast.lon(),
        })
    }
    pub fn contains(self, p: Point) -> bool {
        p.lat() >= self.south
            && p.lat() <= self.north
            && if self.west <= self.east {
                p.lon() >= self.west && p.lon() <= self.east
            } else {
                p.lon() >= self.west || p.lon() <= self.east
            }
    }
    pub fn radius(center: Point, metres: f64) -> Self {
        let angle = (metres / EARTH_RADIUS).min(std::f64::consts::PI);
        // Round bounds outward; the index must never discard a boundary match.
        let delta = angle.to_degrees() + 1e-10;
        let south = (center.lat() - delta).max(-90.0);
        let north = (center.lat() + delta).min(90.0);
        if south == -90.0 || north == 90.0 {
            return Self {
                south,
                north,
                west: -180.0,
                east: 180.0,
            };
        }
        let longitude = (angle.sin() / center.lat().to_radians().cos())
            .clamp(-1.0, 1.0)
            .asin()
            .abs()
            .to_degrees()
            + 1e-10;
        let wrap = |lon: f64| (lon + 180.0).rem_euclid(360.0) - 180.0;
        Self {
            south,
            north,
            west: wrap(center.lon() - longitude),
            east: wrap(center.lon() + longitude),
        }
    }
}

#[derive(Default)]
pub(crate) struct SpatialIndex {
    fields: HashMap<String, BTreeMap<u32, HashSet<NodeId>>>,
}
fn grid(value: f64, low: f64, width: f64) -> u32 {
    (((value - low) / width * 65536.0).floor() as u32).min(65535)
}
fn key(point: Point) -> u32 {
    let x = grid(point.lon(), -180.0, 360.0);
    let y = grid(point.lat(), -90.0, 180.0);
    let mut key = 0;
    for bit in 0..16 {
        key |= ((x >> bit) & 1) << (bit * 2);
        key |= ((y >> bit) & 1) << (bit * 2 + 1);
    }
    key
}
impl SpatialIndex {
    pub fn insert(&mut self, field: &str, point: Point, id: NodeId) {
        self.fields
            .entry(field.to_owned())
            .or_default()
            .entry(key(point))
            .or_default()
            .insert(id);
    }
    pub fn remove(&mut self, field: &str, point: Point, id: NodeId) {
        if let Some(tree) = self.fields.get_mut(field) {
            let key = key(point);
            if let Some(ids) = tree.get_mut(&key) {
                ids.remove(&id);
                if ids.is_empty() {
                    tree.remove(&key);
                }
            }
            if tree.is_empty() {
                self.fields.remove(field);
            }
        }
    }
    pub fn candidates(&self, field: &str, bounds: Bounds) -> HashSet<NodeId> {
        let Some(tree) = self.fields.get(field) else {
            return HashSet::new();
        };
        let mut intervals = Vec::new();
        let mut cover = |west, east| {
            let rect = [
                grid(west, -180.0, 360.0),
                grid(bounds.south, -90.0, 180.0),
                grid(east, -180.0, 360.0),
                grid(bounds.north, -90.0, 180.0),
            ];
            cover_cell(rect, [0, 0], 65536, 0, 0, &mut intervals);
        };
        if bounds.west <= bounds.east {
            cover(bounds.west, bounds.east);
        } else {
            cover(bounds.west, 180.0);
            cover(-180.0, bounds.east);
        }
        let mut ids = HashSet::new();
        for (low, high) in intervals {
            for (_, bucket) in tree.range(low..=high) {
                ids.extend(bucket);
            }
        }
        ids
    }
}
// Coarsen at depth 8 to bound interval count independently of grid resolution.
// Coarsening adds candidates only; predicates always run on original degrees.
fn cover_cell(
    rect: [u32; 4],
    origin: [u32; 2],
    size: u32,
    prefix: u64,
    depth: u32,
    out: &mut Vec<(u32, u32)>,
) {
    let [x, y] = origin;
    if x > rect[2] || y > rect[3] || x + size - 1 < rect[0] || y + size - 1 < rect[1] {
        return;
    }
    if depth == 8
        || (x >= rect[0] && y >= rect[1] && x + size - 1 <= rect[2] && y + size - 1 <= rect[3])
    {
        let shift = 2 * (16 - depth);
        out.push((
            (prefix << shift) as u32,
            (((prefix + 1) << shift) - 1) as u32,
        ));
        return;
    }
    let half = size / 2;
    for child in 0..4 {
        cover_cell(
            rect,
            [x + (child & 1) * half, y + (child >> 1) * half],
            half,
            prefix * 4 + u64::from(child),
            depth + 1,
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{graph::Graph, Value};
    #[test]
    fn morton_index_prunes_and_tracks_replacement() {
        let mut graph = Graph::new();
        for n in 0..10_000 {
            let p = Point::new((n % 180) as f64 - 90.0, (n % 360) as f64 - 180.0).unwrap();
            graph.create_node(
                vec!["Place".into()],
                HashMap::from([("at".into(), Value::Point(p))]),
            );
        }
        let p = Point::new(51.0, -114.0).unwrap();
        let bounds = Bounds::radius(p, 1500.0);
        let id = graph.create_node(
            vec!["Place".into()],
            HashMap::from([("at".into(), Value::Point(p))]),
        );
        let candidates = graph.spatial_candidates("at", bounds);
        assert!(candidates.contains(&id));
        assert!(
            candidates.len() < 100,
            "small query must prune most of the graph"
        );
        graph.update_node(
            id,
            HashMap::from([("at".into(), Value::Point(Point::new(0.0, 0.0).unwrap()))]),
        );
        assert!(!graph.spatial_candidates("at", bounds).contains(&id));
        graph.restore_node(
            id,
            vec!["Place".into()],
            HashMap::from([("at".into(), Value::Point(p))]),
        );
        assert!(graph.spatial_candidates("at", bounds).contains(&id));
        graph.delete_node(id);
        assert!(!graph.spatial_candidates("at", bounds).contains(&id));
    }
}
