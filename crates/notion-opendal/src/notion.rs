use std::collections::HashMap;

use chrono::{DateTime, Utc};
use notion_client::objects::page::{
    DateOrDateTime, DatePropertyValue, Page as NotionPage, PageProperty as NotionPageProperty,
};
use notion_client::objects::rich_text::RichText;
use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(untagged)]
pub enum PropertyValue {
    String(String),
    Number(f64),
    Boolean(bool),
    StringArray(Vec<String>),
    DateTime(DateTime<Utc>),
}

pub fn notion_page_to_properties(page: &NotionPage) -> HashMap<String, PropertyValue> {
    let mut properties = HashMap::new();

    for (name, property) in page.properties.iter() {
        if let Some(value) = property_to_value(property.clone()) {
            properties.insert(name.clone(), value);
        }
    }

    properties
}

pub fn property_to_value(property: NotionPageProperty) -> Option<PropertyValue> {
    match property {
        NotionPageProperty::Title { title, .. } => {
            rich_text_to_string(&title).map(PropertyValue::String)
        }
        NotionPageProperty::RichText { rich_text, .. } => {
            rich_text_to_string(&rich_text).map(PropertyValue::String)
        }
        NotionPageProperty::Select { select, .. } => select
            .and_then(|value| value.name)
            .map(PropertyValue::String),
        NotionPageProperty::Status { status, .. } => status
            .and_then(|value| value.name)
            .map(PropertyValue::String),
        NotionPageProperty::MultiSelect { multi_select, .. } => {
            let values: Vec<String> = multi_select
                .into_iter()
                .filter_map(|item| item.name)
                .collect();

            if values.is_empty() {
                None
            } else {
                Some(PropertyValue::StringArray(values))
            }
        }
        NotionPageProperty::Checkbox { checkbox, .. } => Some(PropertyValue::Boolean(checkbox)),
        NotionPageProperty::Number { number, .. } => number
            .and_then(|value| value.as_f64())
            .map(PropertyValue::Number),
        NotionPageProperty::Url { url, .. } => url.map(PropertyValue::String),
        NotionPageProperty::Email { email, .. } => email.map(PropertyValue::String),
        NotionPageProperty::PhoneNumber { phone_number, .. } => {
            phone_number.map(PropertyValue::String)
        }
        NotionPageProperty::Date { date, .. } => {
            date.and_then(date_to_datetime).map(PropertyValue::DateTime)
        }
        NotionPageProperty::CreatedTime { created_time, .. } => {
            Some(PropertyValue::DateTime(created_time))
        }
        NotionPageProperty::LastEditedTime {
            last_edited_time, ..
        } => last_edited_time.map(PropertyValue::DateTime),
        NotionPageProperty::People { people, .. } => {
            let names: Vec<String> = people.into_iter().filter_map(|user| user.name).collect();
            (!names.is_empty()).then(|| PropertyValue::StringArray(names))
        }
        _ => None,
    }
}

pub fn apply_frontmatter(properties: &HashMap<String, PropertyValue>, markdown: &str) -> String {
    if properties.is_empty() {
        return markdown.to_string();
    }

    let mut entries: Vec<_> = properties.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut frontmatter = String::from("---\n");
    for (key, value) in entries {
        let rendered = property_value_to_string(value);
        let escaped = rendered
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('"', "\\\"");
        frontmatter.push_str(&format!("{key}: \"{escaped}\"\n"));
    }
    frontmatter.push_str("---\n\n");
    frontmatter.push_str(markdown);
    frontmatter
}

pub fn property_value_to_string(value: &PropertyValue) -> String {
    match value {
        PropertyValue::String(value) => value.clone(),
        PropertyValue::Number(value) => value.to_string(),
        PropertyValue::Boolean(value) => value.to_string(),
        PropertyValue::StringArray(values) => values.join(", "),
        PropertyValue::DateTime(value) => value.to_rfc3339(),
    }
}

pub fn rich_text_to_string(text: &[RichText]) -> Option<String> {
    let combined = text
        .iter()
        .filter_map(|item| item.plain_text())
        .collect::<String>();

    let trimmed = combined.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn date_to_datetime(date: DatePropertyValue) -> Option<DateTime<Utc>> {
    date.start.and_then(date_or_datetime_to_datetime)
}

pub fn date_or_datetime_to_datetime(date: DateOrDateTime) -> Option<DateTime<Utc>> {
    match date {
        DateOrDateTime::Date(date) => date
            .and_hms_opt(0, 0, 0)
            .map(|dt| DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc)),
        DateOrDateTime::DateTime(date_time) => Some(date_time),
    }
}
