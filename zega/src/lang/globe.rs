//! The `globe` display view (APS 9) and the `String<iso2>` country code it joins
//! to Natural Earth outlines. Kept apart from the display block's own parsing so
//! moving `display` into `query`/`then` moves these functions unchanged.
use super::node_display::{AttributeValue, DisplayAttribute};
use super::*;

/// ISO 3166-1 alpha-2: the 249 officially assigned codes. User-assigned codes
/// (`XK`, `AA`, `ZZ`, ...) and exceptional reservations (`UK`, `EU`) are not.
pub(super) const ISO2_CODES: &str = "\
AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ \
BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BV BW BY BZ \
CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ \
DE DJ DK DM DO DZ \
EC EE EG EH ER ES ET \
FI FJ FK FM FO FR \
GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY \
HK HM HN HR HT HU \
ID IE IL IM IN IO IQ IR IS IT \
JE JM JO JP \
KE KG KH KI KM KN KP KR KW KY KZ \
LA LB LC LI LK LR LS LT LU LV LY \
MA MC MD ME MF MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ \
NA NC NE NF NG NI NL NO NP NR NU NZ \
OM \
PA PE PF PG PH PK PL PM PN PR PS PT PW PY \
QA \
RE RO RS RU RW \
SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ \
TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW TZ \
UA UG UM US UY UZ \
VA VC VE VG VI VN VU \
WF WS \
YE YT \
ZA ZM ZW";

pub(crate) const ISO2_HELP: &str =
    "write an assigned ISO 3166-1 alpha-2 country code in capitals, e.g. `CA`, `GB` or `JP`";

/// `String<iso2>`: exactly two capitals naming an assigned ISO 3166-1 country.
pub(crate) fn valid_iso2(value: &str) -> bool {
    value.len() == 2 && ISO2_CODES.split_ascii_whitespace().any(|code| code == value)
}

pub(crate) const ZOOM: std::ops::RangeInclusive<f64> = 0.0..=22.0;
pub(crate) const TILT: std::ops::RangeInclusive<f64> = 0.0..=85.0;

/// Where the globe starts. Every setting is optional in source; the checked
/// contract is always complete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlobeCamera {
    /// MapLibre zoom, 0 to 22. The globe becomes the flat map near zoom 12.
    pub zoom: f64,
    /// Degrees from straight down, 0 to 85.
    pub tilt: f64,
    pub center: GlobeCenter,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlobeCenter {
    pub lat: f64,
    pub lon: f64,
}

impl Default for GlobeCamera {
    fn default() -> Self {
        Self {
            zoom: 1.5,
            tilt: 0.0,
            center: GlobeCenter { lat: 20.0, lon: 0.0 },
        }
    }
}

/// Settings written between a view name and its type list: `globe(@zoom: 2)`.
pub(super) struct ViewSettings {
    pub(super) attributes: Vec<DisplayAttribute>,
    pub(super) span: Span,
}

impl Parser<'_> {
    /// `(@name: value, ...)` after a view name, if present.
    pub(super) fn parse_view_settings(&mut self) -> Result<Option<ViewSettings>> {
        self.skip();
        if !self.src[self.i..].starts_with('(') {
            return Ok(None);
        }
        let start = self.i;
        let attributes = self.parse_display_attributes()?;
        Ok(Some(ViewSettings {
            attributes,
            span: self.span_bytes(start, self.i),
        }))
    }
}

/// A globe lists types with a country code or coordinates.
pub(super) fn eligible(ty: &TypeDef) -> bool {
    ty.fields
        .iter()
        .any(|field| matches!(field, Field::Prop { ty, .. } if ty == "String<iso2>"))
        || has_coordinates(ty)
}

pub(super) fn has_coordinates(ty: &TypeDef) -> bool {
    ty.fields
        .iter()
        .any(|field| matches!(field, Field::Prop { ty, .. } if ty == "Point"))
        || ["lat", "lon"].iter().all(|coordinate| {
            ty.fields.iter().any(|field| {
                matches!(field, Field::Prop { name, ty, .. } if name == coordinate && ty == "Float")
            })
        })
}

/// Check the settings of one view. Only `globe` takes any.
pub(super) fn check_settings(
    kind: ViewKind,
    settings: Option<ViewSettings>,
) -> Result<Option<GlobeCamera>> {
    if kind != ViewKind::Globe {
        return match settings {
            Some(ViewSettings { span, .. }) => Err(Error::at(
                span,
                format!("display view {} takes no settings", view_name(kind)),
            )
            .with_help("only `globe(@zoom: …, @tilt: …, @center: @point(…))` has settings")),
            None => Ok(None),
        };
    }
    let mut camera = GlobeCamera::default();
    let Some(ViewSettings { attributes, .. }) = settings else {
        return Ok(Some(camera));
    };
    let mut seen = std::collections::HashSet::new();
    for DisplayAttribute {
        name,
        span,
        value,
        value_span,
    } in &attributes
    {
        if !seen.insert(name) {
            return Err(Error::at(*span, format!("duplicate globe setting @{name}"))
                .with_help("set each globe setting once"));
        }
        let number = |range: &std::ops::RangeInclusive<f64>| match value {
            AttributeValue::Literal(Json::Number(n)) => n.as_f64().filter(|n| range.contains(n)),
            _ => None,
        };
        match name.as_str() {
            "zoom" => {
                camera.zoom = number(&ZOOM).ok_or_else(|| {
                    Error::at(*value_span, "@zoom must be a number from 0 to 22")
                        .with_help("1.5 shows the whole globe; the globe becomes the flat map near 12")
                })?
            }
            "tilt" => {
                camera.tilt = number(&TILT).ok_or_else(|| {
                    Error::at(*value_span, "@tilt must be a number of degrees from 0 to 85")
                        .with_help("0 looks straight down; `@tilt: 20` leans toward the horizon")
                })?
            }
            "center" => {
                let point = match value {
                    AttributeValue::Literal(json) => Point::from_json(json).ok(),
                    _ => None,
                };
                let point = point.ok_or_else(|| {
                    Error::at(*value_span, "@center must be a @point(latitude, longitude)")
                        .with_help("write `@center: @point(51.05, -114.07)`")
                })?;
                camera.center = GlobeCenter {
                    lat: point.lat(),
                    lon: point.lon(),
                };
            }
            _ => {
                return Err(Error::at(*span, format!("unknown globe setting @{name}"))
                    .with_help("use `@zoom`, `@tilt` or `@center`"))
            }
        }
    }
    Ok(Some(camera))
}

pub(super) fn view_name(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::Graph => "graph",
        ViewKind::Table => "table",
        ViewKind::Map => "map",
        ViewKind::Globe => "globe",
        ViewKind::Timeline => "timeline",
        ViewKind::Vector2d => "vector2d",
        ViewKind::Vector3d => "vector3d",
    }
}
