use crate::schema::DeclarationKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WireSchema {
    TelegramApi,
    MtprotoApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireConstructor {
    pub schema: WireSchema,
    pub name: &'static str,
    pub id: u32,
    pub kind: DeclarationKind,
    pub result_type: &'static str,
}

include!("schema_ids.rs");

pub fn wire_constructors_by_id(id: u32) -> impl Iterator<Item = &'static WireConstructor> {
    WIRE_CONSTRUCTORS
        .iter()
        .filter(move |constructor| constructor.id == id)
}

pub fn wire_constructor_by_name(
    schema: WireSchema,
    name: &str,
) -> Option<&'static WireConstructor> {
    WIRE_CONSTRUCTORS
        .iter()
        .find(|constructor| constructor.schema == schema && constructor.name == name)
}
