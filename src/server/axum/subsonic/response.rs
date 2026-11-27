//! The OpenSubsonic response envelope.
//!
//! Responses are described once, as a tree of [`Element`]s, and rendered as XML (the default),
//! JSON or JSONP depending on the `f` request parameter. In the tree, scalar values are
//! attributes (XML attributes, JSON properties), and nested values are child elements.

use axum::{
	http::{header, HeaderValue},
	response::{IntoResponse, Response},
};
use serde_json::{Map, Number, Value};

/// The version of the Subsonic API implemented (the OpenSubsonic reference is 1.16.1).
pub const API_VERSION: &str = "1.16.1";
pub const API_VERSION_PARTS: (u32, u32, u32) = (1, 16, 1);
const SERVER_TYPE: &str = "polaris";
const XML_NAMESPACE: &str = "http://subsonic.org/restapi";

#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
	Str(String),
	Bool(bool),
	Int(i64),
	Float(f64),
}

impl From<&str> for Scalar {
	fn from(v: &str) -> Self {
		Scalar::Str(v.to_owned())
	}
}
impl From<String> for Scalar {
	fn from(v: String) -> Self {
		Scalar::Str(v)
	}
}
impl From<bool> for Scalar {
	fn from(v: bool) -> Self {
		Scalar::Bool(v)
	}
}
impl From<i64> for Scalar {
	fn from(v: i64) -> Self {
		Scalar::Int(v)
	}
}
impl From<i32> for Scalar {
	fn from(v: i32) -> Self {
		Scalar::Int(v.into())
	}
}
impl From<u32> for Scalar {
	fn from(v: u32) -> Self {
		Scalar::Int(v.into())
	}
}
impl From<f64> for Scalar {
	fn from(v: f64) -> Self {
		Scalar::Float(v)
	}
}

#[derive(Clone, Debug, PartialEq)]
pub enum Child {
	/// A single nested object (`"license": {...}` / `<license .../>`).
	Object(&'static str, Element),
	/// A list of objects. XML repeats the element, JSON uses an array. Empty lists are omitted.
	List(&'static str, Vec<Element>),
	/// A list the specification requires: JSON has the array even when it is empty. (XML has
	/// nothing for an empty list either way.)
	RequiredList(&'static str, Vec<Element>),
	/// A list of plain values (`"versions": [1]` / `<versions>1</versions>`).
	Scalars(&'static str, Vec<Scalar>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
	attrs: Vec<(&'static str, Scalar)>,
	children: Vec<Child>,
	/// Text content: the element's text in XML, a `value` property in JSON.
	text: Option<String>,
}

impl Element {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn attr(mut self, name: &'static str, value: impl Into<Scalar>) -> Self {
		self.attrs.push((name, value.into()));
		self
	}

	// Used by the endpoints returning optional fields, which come after the system endpoints.
	#[allow(dead_code)]
	pub fn attr_opt(self, name: &'static str, value: Option<impl Into<Scalar>>) -> Self {
		match value {
			Some(v) => self.attr(name, v),
			None => self,
		}
	}

	pub fn text(mut self, text: impl Into<String>) -> Self {
		self.text = Some(text.into());
		self
	}

	pub fn object(mut self, name: &'static str, element: Element) -> Self {
		self.children.push(Child::Object(name, element));
		self
	}

	pub fn list(mut self, name: &'static str, elements: Vec<Element>) -> Self {
		self.children.push(Child::List(name, elements));
		self
	}

	pub fn required_list(mut self, name: &'static str, elements: Vec<Element>) -> Self {
		self.children.push(Child::RequiredList(name, elements));
		self
	}

	pub fn scalars<T: Into<Scalar>>(mut self, name: &'static str, values: Vec<T>) -> Self {
		let values = values.into_iter().map(Into::into).collect();
		self.children.push(Child::Scalars(name, values));
		self
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Format {
	Xml,
	Json,
	Jsonp(String),
}

/// A Subsonic error. These are reported inside a normal (HTTP 200) response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubsonicError {
	pub code: u32,
	pub message: String,
}

impl SubsonicError {
	fn new(code: u32, message: impl Into<String>) -> Self {
		Self {
			code,
			message: message.into(),
		}
	}

	pub fn generic(message: impl Into<String>) -> Self {
		Self::new(0, message)
	}
	pub fn missing_parameter(name: &str) -> Self {
		Self::new(10, format!("Required parameter is missing: {name}"))
	}
	pub fn client_must_upgrade() -> Self {
		Self::new(
			20,
			"Incompatible Subsonic REST protocol version. Client must upgrade.",
		)
	}
	pub fn server_must_upgrade() -> Self {
		Self::new(
			30,
			"Incompatible Subsonic REST protocol version. Server must upgrade.",
		)
	}
	pub fn wrong_credentials() -> Self {
		Self::new(40, "Wrong username or password")
	}
	pub fn token_auth_unsupported() -> Self {
		Self::new(
			41,
			"Token authentication is not supported. Use an API key (apiKey).",
		)
	}
	pub fn mechanism_unsupported(message: impl Into<String>) -> Self {
		Self::new(42, message)
	}
	pub fn conflicting_mechanisms() -> Self {
		Self::new(
			43,
			"Multiple conflicting authentication mechanisms provided",
		)
	}
	pub fn not_authorized() -> Self {
		Self::new(50, "User is not authorized for the given operation")
	}
	pub fn not_found() -> Self {
		Self::new(70, "The requested data was not found")
	}
	pub fn invalid_parameter(name: &str) -> Self {
		Self::new(0, format!("Invalid value for parameter: {name}"))
	}
	pub fn invalid_api_key() -> Self {
		Self::new(44, "Invalid API key")
	}
}

/// A complete reply, ready to be sent.
#[derive(Clone, Debug)]
pub struct Reply {
	format: Format,
	root: Element,
}

fn envelope(status: &'static str, body: Element) -> Element {
	let mut root = Element::new()
		.attr("status", status)
		.attr("version", API_VERSION)
		.attr("type", SERVER_TYPE)
		.attr("serverVersion", env!("CARGO_PKG_VERSION"))
		.attr("openSubsonic", true);
	root.attrs.extend(body.attrs);
	root.children.extend(body.children);
	root
}

impl Reply {
	pub fn ok(format: Format, body: Element) -> Self {
		Self {
			format,
			root: envelope("ok", body),
		}
	}

	pub fn error(format: Format, error: SubsonicError) -> Self {
		let body = Element::new().object(
			"error",
			Element::new()
				.attr("code", error.code)
				.attr("message", error.message),
		);
		Self {
			format,
			root: envelope("failed", body),
		}
	}

	pub fn render(&self) -> (String, &'static str) {
		match &self.format {
			Format::Xml => (render_xml(&self.root), "application/xml; charset=utf-8"),
			Format::Json => (render_json(&self.root), "application/json; charset=utf-8"),
			Format::Jsonp(callback) => (
				format!("{callback}({});", render_json(&self.root)),
				"application/javascript; charset=utf-8",
			),
		}
	}
}

impl IntoResponse for Reply {
	fn into_response(self) -> Response {
		let (body, content_type) = self.render();
		let mut response = body.into_response();
		let headers = response.headers_mut();
		headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
		// Responses may carry user data, and requests carry credentials in their URL.
		headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
		headers.insert(
			header::X_CONTENT_TYPE_OPTIONS,
			HeaderValue::from_static("nosniff"),
		);
		response
	}
}

/// Whether `name` is acceptable as a JSONP callback. Anything else could be used to inject
/// script, so it is rejected rather than escaped.
pub fn is_valid_jsonp_callback(name: &str) -> bool {
	!name.is_empty()
		&& name.len() <= 128
		&& !name.starts_with(|c: char| c.is_ascii_digit())
		&& name
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.')
		&& !name.starts_with('.')
		&& !name.ends_with('.')
		&& !name.contains("..")
}

// JSON

fn scalar_to_json(scalar: &Scalar) -> Value {
	match scalar {
		Scalar::Str(s) => Value::String(s.clone()),
		Scalar::Bool(b) => Value::Bool(*b),
		Scalar::Int(i) => Value::Number((*i).into()),
		Scalar::Float(f) => Number::from_f64(*f)
			.map(Value::Number)
			.unwrap_or(Value::Null),
	}
}

fn element_to_json(element: &Element) -> Value {
	let mut map = Map::new();
	for (name, value) in &element.attrs {
		map.insert((*name).to_owned(), scalar_to_json(value));
	}
	if let Some(text) = &element.text {
		map.insert("value".to_owned(), Value::String(text.clone()));
	}
	for child in &element.children {
		match child {
			Child::Object(name, e) => {
				map.insert((*name).to_owned(), element_to_json(e));
			}
			Child::List(name, elements) if !elements.is_empty() => {
				let items = elements.iter().map(element_to_json).collect();
				map.insert((*name).to_owned(), Value::Array(items));
			}
			Child::RequiredList(name, elements) => {
				let items = elements.iter().map(element_to_json).collect();
				map.insert((*name).to_owned(), Value::Array(items));
			}
			Child::Scalars(name, values) if !values.is_empty() => {
				let items = values.iter().map(scalar_to_json).collect();
				map.insert((*name).to_owned(), Value::Array(items));
			}
			_ => (),
		}
	}
	Value::Object(map)
}

fn render_json(root: &Element) -> String {
	let mut wrapper = Map::new();
	wrapper.insert("subsonic-response".to_owned(), element_to_json(root));
	Value::Object(wrapper).to_string()
}

// XML

/// Escapes text for use in an attribute value (`in_attribute`) or element content, dropping
/// characters that XML 1.0 cannot represent at all.
///
/// XML parsers normalize raw tabs and line breaks in attribute values to spaces, and line breaks
/// in content to `\n`, so those are written as character references to survive intact.
fn escape_xml(text: &str, in_attribute: bool, out: &mut String) {
	for c in text.chars() {
		match c {
			'&' => out.push_str("&amp;"),
			'<' => out.push_str("&lt;"),
			'>' => out.push_str("&gt;"),
			'"' => out.push_str("&quot;"),
			'\'' => out.push_str("&apos;"),
			'\t' if in_attribute => out.push_str("&#9;"),
			'\n' if in_attribute => out.push_str("&#10;"),
			'\r' => out.push_str("&#13;"),
			'\t' | '\n' => out.push(c),
			c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => (),
			c => out.push(c),
		}
	}
}

fn scalar_to_xml(scalar: &Scalar) -> String {
	match scalar {
		Scalar::Str(s) => s.clone(),
		Scalar::Bool(b) => b.to_string(),
		Scalar::Int(i) => i.to_string(),
		Scalar::Float(f) => f.to_string(),
	}
}

fn write_xml(out: &mut String, name: &str, element: &Element, namespace: bool) {
	out.push('<');
	out.push_str(name);
	if namespace {
		out.push_str(&format!(" xmlns=\"{XML_NAMESPACE}\""));
	}
	for (attr, value) in &element.attrs {
		out.push(' ');
		out.push_str(attr);
		out.push_str("=\"");
		escape_xml(&scalar_to_xml(value), true, out);
		out.push('"');
	}

	let has_children = element.children.iter().any(|c| match c {
		Child::Object(..) => true,
		Child::List(_, v) | Child::RequiredList(_, v) => !v.is_empty(),
		Child::Scalars(_, v) => !v.is_empty(),
	});
	if !has_children && element.text.is_none() {
		out.push_str("/>");
		return;
	}
	out.push('>');
	if let Some(text) = &element.text {
		escape_xml(text, false, out);
	}
	for child in &element.children {
		match child {
			Child::Object(child_name, e) => write_xml(out, child_name, e, false),
			Child::List(child_name, elements) | Child::RequiredList(child_name, elements) => {
				for e in elements {
					write_xml(out, child_name, e, false);
				}
			}
			Child::Scalars(child_name, values) => {
				for value in values {
					out.push('<');
					out.push_str(child_name);
					out.push('>');
					escape_xml(&scalar_to_xml(value), false, out);
					out.push_str("</");
					out.push_str(child_name);
					out.push('>');
				}
			}
		}
	}
	out.push_str("</");
	out.push_str(name);
	out.push('>');
}

fn render_xml(root: &Element) -> String {
	let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
	write_xml(&mut out, "subsonic-response", root, true);
	out
}

#[cfg(test)]
mod test {
	use super::*;

	fn sample() -> Element {
		Element::new()
			.attr("name", "a & b <c> \"d\" 'e'")
			.attr("count", 3)
			.attr("ratio", 0.5)
			.attr("flag", true)
			.object("inner", Element::new().attr("x", "y"))
			.list(
				"item",
				vec![
					Element::new().attr("id", "1"),
					Element::new().attr("id", "2"),
				],
			)
			.list("empty", vec![])
			.scalars("versions", vec![1, 2])
	}

	#[test]
	fn json_uses_typed_properties_arrays_and_omits_empty_lists() {
		let (body, content_type) = Reply::ok(Format::Json, sample()).render();
		assert_eq!(content_type, "application/json; charset=utf-8");
		let json: Value = serde_json::from_str(&body).unwrap();
		let root = &json["subsonic-response"];
		assert_eq!(root["status"], "ok");
		assert_eq!(root["version"], API_VERSION);
		assert_eq!(root["type"], "polaris");
		assert_eq!(root["openSubsonic"], true);
		assert_eq!(root["name"], "a & b <c> \"d\" 'e'");
		assert_eq!(root["count"], 3);
		assert_eq!(root["ratio"], 0.5);
		assert_eq!(root["flag"], true);
		assert_eq!(root["inner"]["x"], "y");
		assert_eq!(root["item"][1]["id"], "2");
		assert_eq!(root["versions"], serde_json::json!([1, 2]));
		assert!(root.get("empty").is_none());
	}

	#[test]
	fn xml_uses_attributes_repeated_elements_and_namespace() {
		let (body, content_type) = Reply::ok(Format::Xml, sample()).render();
		assert_eq!(content_type, "application/xml; charset=utf-8");
		assert!(body.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?><subsonic-response"));

		let doc = roxmltree::Document::parse(&body).unwrap();
		let root = doc.root_element();
		assert_eq!(root.tag_name().namespace(), Some(XML_NAMESPACE));
		assert_eq!(root.attribute("status"), Some("ok"));
		assert_eq!(root.attribute("openSubsonic"), Some("true"));
		assert_eq!(root.attribute("name"), Some("a & b <c> \"d\" 'e'"));
		assert_eq!(root.attribute("count"), Some("3"));
		assert_eq!(root.attribute("flag"), Some("true"));
		let items: Vec<_> = root.children().filter(|n| n.has_tag_name("item")).collect();
		assert_eq!(items.len(), 2);
		assert_eq!(items[1].attribute("id"), Some("2"));
		let versions: Vec<_> = root
			.children()
			.filter(|n| n.has_tag_name("versions"))
			.map(|n| n.text().unwrap())
			.collect();
		assert_eq!(versions, vec!["1", "2"]);
		assert!(!root.children().any(|n| n.has_tag_name("empty")));
	}

	#[test]
	fn xml_attribute_order_follows_the_spec_example() {
		let (body, _) = Reply::ok(Format::Xml, Element::new()).render();
		// Skip the XML declaration, which has a `version` attribute of its own.
		let body = &body[body.find("<subsonic-response").unwrap()..];
		let status = body.find("status=").unwrap();
		let version = body.find(" version=").unwrap();
		let kind = body.find(" type=").unwrap();
		let server_version = body.find(" serverVersion=").unwrap();
		let open_subsonic = body.find(" openSubsonic=").unwrap();
		assert!(status < version && version < kind && kind < server_version);
		assert!(server_version < open_subsonic);
		assert!(body.ends_with("/>"));
	}

	#[test]
	fn xml_drops_characters_it_cannot_represent_and_stays_well_formed() {
		let hostile = Element::new().attr("v", "a\u{0}b\u{1}c\u{FFFE}d\te\nf\"><script>&");
		let (body, _) = Reply::ok(Format::Xml, hostile).render();
		let doc = roxmltree::Document::parse(&body).unwrap();
		assert_eq!(
			doc.root_element().attribute("v"),
			Some("abcd\te\nf\"><script>&")
		);
		assert!(!body.contains("<script>"));
	}

	#[test]
	fn xml_keeps_tabs_and_line_breaks_in_attributes_and_text() {
		let tricky = "line1\nline2\r\nline3\ttab";
		let body = Element::new()
			.attr("v", tricky)
			.scalars("text", vec![tricky.to_owned()]);
		let (body, _) = Reply::ok(Format::Xml, body).render();
		let doc = roxmltree::Document::parse(&body).unwrap();
		let root = doc.root_element();
		assert_eq!(root.attribute("v"), Some(tricky));
		let text = root.children().find(|n| n.has_tag_name("text")).unwrap();
		// Content line breaks are normalized to \n by parsers, except escaped carriage returns.
		assert_eq!(text.text(), Some(tricky));
	}

	#[test]
	fn required_lists_are_arrays_in_json_even_when_empty() {
		let body = Element::new()
			.required_list("entry", vec![])
			.list("optional", vec![])
			.required_list("full", vec![Element::new().attr("id", "1")]);
		let (json, _) = Reply::ok(Format::Json, Element::new().object("x", body.clone())).render();
		let json: Value = serde_json::from_str(&json).unwrap();
		assert_eq!(
			json["subsonic-response"]["x"]["entry"],
			serde_json::json!([])
		);
		assert!(json["subsonic-response"]["x"].get("optional").is_none());
		assert_eq!(json["subsonic-response"]["x"]["full"][0]["id"], "1");

		let (xml, _) = Reply::ok(Format::Xml, Element::new().object("x", body)).render();
		let doc = roxmltree::Document::parse(&xml).unwrap();
		let x = doc
			.root_element()
			.children()
			.find(|n| n.has_tag_name("x"))
			.unwrap();
		assert_eq!(x.children().filter(|n| n.has_tag_name("full")).count(), 1);
		assert!(!x.children().any(|n| n.has_tag_name("entry")));
	}

	#[test]
	fn text_content_is_a_value_property_in_json_and_a_text_node_in_xml() {
		let lyrics = Element::new()
			.attr("artist", "Someone")
			.text("line 1\nline <2> & more");
		let (body, _) = Reply::ok(
			Format::Json,
			Element::new().object("lyrics", lyrics.clone()),
		)
		.render();
		let json: Value = serde_json::from_str(&body).unwrap();
		assert_eq!(
			json["subsonic-response"]["lyrics"]["value"],
			"line 1\nline <2> & more"
		);
		assert_eq!(json["subsonic-response"]["lyrics"]["artist"], "Someone");

		let (body, _) = Reply::ok(Format::Xml, Element::new().object("lyrics", lyrics)).render();
		let doc = roxmltree::Document::parse(&body).unwrap();
		let node = doc
			.root_element()
			.children()
			.find(|n| n.has_tag_name("lyrics"))
			.unwrap();
		assert_eq!(node.attribute("artist"), Some("Someone"));
		assert_eq!(node.text(), Some("line 1\nline <2> & more"));
	}

	#[test]
	fn errors_have_status_failed_and_an_error_element() {
		let (body, _) = Reply::error(Format::Json, SubsonicError::invalid_api_key()).render();
		let json: Value = serde_json::from_str(&body).unwrap();
		assert_eq!(json["subsonic-response"]["status"], "failed");
		assert_eq!(json["subsonic-response"]["error"]["code"], 44);
		assert!(json["subsonic-response"]["error"]["message"].is_string());

		let (body, _) = Reply::error(Format::Xml, SubsonicError::missing_parameter("v")).render();
		let doc = roxmltree::Document::parse(&body).unwrap();
		let error = doc
			.root_element()
			.children()
			.find(|n| n.has_tag_name("error"))
			.unwrap();
		assert_eq!(error.attribute("code"), Some("10"));
	}

	#[test]
	fn jsonp_wraps_the_json_in_the_callback() {
		let (body, content_type) =
			Reply::ok(Format::Jsonp("my.callback_1".to_owned()), Element::new()).render();
		assert_eq!(content_type, "application/javascript; charset=utf-8");
		assert!(body.starts_with("my.callback_1({\"subsonic-response\":"));
		assert!(body.ends_with(");"));
	}

	#[test]
	fn jsonp_callbacks_are_restricted_to_safe_identifiers() {
		for good in ["cb", "_cb", "$", "a.b.c", "cb1", "Foo_Bar$2"] {
			assert!(is_valid_jsonp_callback(good), "{good}");
		}
		for bad in [
			"",
			"1cb",
			"a b",
			"a-b",
			"alert(1)//",
			"a;b",
			"</script>",
			".a",
			"a.",
			"a..b",
			"a\nb",
			"a'b",
			&"x".repeat(129),
		] {
			assert!(!is_valid_jsonp_callback(bad), "{bad:?}");
		}
	}

	#[test]
	fn responses_are_not_cacheable_and_not_sniffable() {
		let response = Reply::ok(Format::Json, Element::new()).into_response();
		assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
		assert_eq!(
			response.headers()[header::X_CONTENT_TYPE_OPTIONS],
			"nosniff"
		);
		assert_eq!(response.status(), 200);
	}
}
