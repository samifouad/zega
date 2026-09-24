use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeShape {
    #[default]
    Circle,
    Document,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDisplay {
    pub shape: NodeShape,
    pub size: u8,
    pub image: Option<String>,
}

impl Default for NodeDisplay {
    fn default() -> Self {
        Self {
            shape: NodeShape::Circle,
            size: 1,
            image: None,
        }
    }
}

pub(super) struct DisplayAttribute {
    name: String,
    span: Span,
    value: AttributeValue,
    value_span: Span,
}

enum AttributeValue {
    Name(String),
    Field(String),
    Literal(Json),
}

impl Parser<'_> {
    pub(super) fn parse_display_attributes(&mut self) -> Result<Vec<DisplayAttribute>> {
        let mut attributes = Vec::new();
        if !self.eat("(") {
            return Ok(attributes);
        }
        while !self.eat(")") {
            self.skip();
            let start = self.i;
            let at = self.eat("@");
            let (name, _) = self.ident()?;
            let span = self.span_bytes(start, self.i);
            if !at {
                return Err(
                    Error::at(span, format!("display attribute {name} needs @{name}"))
                        .with_help(format!("write `@{name}: …`; `@` names language attributes")),
                );
            }
            self.expect(":")?;
            self.skip();
            let start = self.i;
            let value = if self.eat("&") {
                AttributeValue::Field(self.ident()?.0)
            } else if self.looking_at_ident() {
                AttributeValue::Name(self.ident()?.0)
            } else {
                AttributeValue::Literal(self.parse_value()?)
            };
            attributes.push(DisplayAttribute {
                name,
                span,
                value,
                value_span: self.span_bytes(start, self.i),
            });
            if self.eat(")") {
                break;
            }
            self.expect(",")?;
        }
        Ok(attributes)
    }
}

pub(super) fn check_attributes(
    ty: &TypeDef,
    attributes: &[DisplayAttribute],
) -> Result<NodeDisplay> {
    let mut config = NodeDisplay::default();
    let mut seen = std::collections::HashSet::new();
    for attribute in attributes {
        let DisplayAttribute {
            name,
            span,
            value,
            value_span,
        } = attribute;
        if !seen.insert(name) {
            return Err(
                Error::at(*span, format!("duplicate display attribute @{name}"))
                    .with_help("set each attribute once per type"),
            );
        }
        match name.as_str() {
            "shape" => {
                config.shape = match value {
                    AttributeValue::Name(name) if name == "circle" => NodeShape::Circle,
                    AttributeValue::Name(name) if name == "document" => NodeShape::Document,
                    _ => {
                        return Err(Error::at(*value_span, "@shape must be circle or document")
                            .with_help("write `@shape: circle` or `@shape: document`"))
                    }
                }
            }
            "size" => {
                config.size = match value {
                    AttributeValue::Literal(Json::Number(n))
                        if n.as_u64().is_some_and(|n| (1..=3).contains(&n)) =>
                    {
                        n.as_u64().unwrap() as u8
                    }
                    _ => {
                        return Err(Error::at(*value_span, "@size must be 1, 2 or 3")
                            .with_help("write `@size: 1`, `@size: 2` or `@size: 3`"))
                    }
                }
            }
            "image" => {
                let AttributeValue::Field(field) = value else {
                    return Err(Error::at(*value_span, "@image needs a field reference")
                        .with_help("write `@image: &scan` and declare `scan: String<url>`"));
                };
                if !ty.fields.iter().any(|f| matches!(f, Field::Prop { name, ty, .. } if name == field && ty == "String<url>")) {
                    return Err(Error::at(*value_span, format!("@image needs {}.{field} to be String<url>", ty.name))
                        .with_help(format!("declare `{field}: String<url>` on {}", ty.name)));
                }
                config.image = Some(field.clone());
            }
            _ => {
                return Err(
                    Error::at(*span, format!("unknown display attribute @{name}"))
                        .with_help("use `@shape`, `@image` or `@size`"),
                )
            }
        }
    }
    Ok(config)
}
