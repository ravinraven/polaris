//! Request parameters. Subsonic passes everything as query parameters, optionally as an
//! `application/x-www-form-urlencoded` body too (the `formPost` extension). Some parameters
//! can be repeated (eg. `songId=1&songId=2`).

use super::response::SubsonicError;

#[derive(Clone, Debug, Default)]
pub struct Params(Vec<(String, String)>);

impl Params {
	pub fn parse(query: Option<&str>, form_body: Option<&[u8]>) -> Self {
		let mut pairs = Vec::new();
		if let Some(query) = query {
			pairs.extend(form_urlencoded::parse(query.as_bytes()).into_owned());
		}
		if let Some(body) = form_body {
			pairs.extend(form_urlencoded::parse(body).into_owned());
		}
		Self(pairs)
	}

	/// The first value of a parameter.
	pub fn get(&self, name: &str) -> Option<&str> {
		self.0
			.iter()
			.find(|(k, _)| k == name)
			.map(|(_, v)| v.as_str())
	}

	/// Every value of a parameter, in order.
	pub fn get_all(&self, name: &str) -> Vec<&str> {
		self.0
			.iter()
			.filter(|(k, _)| k == name)
			.map(|(_, v)| v.as_str())
			.collect()
	}

	/// A parameter that must be present and non-empty.
	pub fn required(&self, name: &str) -> Result<&str, SubsonicError> {
		match self.get(name) {
			Some(value) if !value.is_empty() => Ok(value),
			_ => Err(SubsonicError::missing_parameter(name)),
		}
	}

	/// An optional integer parameter. Present but not a number is an error.
	pub fn int(&self, name: &str) -> Result<Option<i64>, SubsonicError> {
		match self.get(name) {
			None | Some("") => Ok(None),
			Some(v) => v
				.trim()
				.parse::<i64>()
				.map(Some)
				.map_err(|_| SubsonicError::invalid_parameter(name)),
		}
	}

	pub fn required_int(&self, name: &str) -> Result<i64, SubsonicError> {
		self.required(name)?;
		self.int(name)?
			.ok_or_else(|| SubsonicError::missing_parameter(name))
	}

	/// An optional boolean parameter (`true` or `false`).
	pub fn bool(&self, name: &str) -> Result<Option<bool>, SubsonicError> {
		match self.get(name) {
			None | Some("") => Ok(None),
			Some(v) if v.eq_ignore_ascii_case("true") => Ok(Some(true)),
			Some(v) if v.eq_ignore_ascii_case("false") => Ok(Some(false)),
			Some(_) => Err(SubsonicError::invalid_parameter(name)),
		}
	}

	/// Every value of a repeated integer parameter.
	pub fn ints(&self, name: &str) -> Result<Vec<i64>, SubsonicError> {
		self.get_all(name)
			.into_iter()
			.map(|v| {
				v.trim()
					.parse::<i64>()
					.map_err(|_| SubsonicError::invalid_parameter(name))
			})
			.collect()
	}

	pub fn count(&self, name: &str) -> usize {
		self.0.iter().filter(|(k, _)| k == name).count()
	}

	pub fn contains(&self, name: &str) -> bool {
		self.count(name) > 0
	}
}

#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn reads_query_and_body_with_decoding() {
		let params = Params::parse(
			Some("u=al%20ice&p=a%2Bb+c&f=json"),
			Some(b"c=my%26app&empty="),
		);
		assert_eq!(params.get("u"), Some("al ice"));
		assert_eq!(params.get("p"), Some("a+b c"));
		assert_eq!(params.get("f"), Some("json"));
		assert_eq!(params.get("c"), Some("my&app"));
		assert_eq!(params.get("empty"), Some(""));
		assert_eq!(params.get("missing"), None);
		assert!(params.contains("empty"));
		assert!(!params.contains("missing"));
	}

	#[test]
	fn repeated_parameters_keep_their_order() {
		let params = Params::parse(Some("id=3&x=1&id=1&id=2"), None);
		assert_eq!(params.get_all("id"), vec!["3", "1", "2"]);
		assert_eq!(params.get("id"), Some("3"));
		assert_eq!(params.count("id"), 3);
		assert_eq!(params.count("x"), 1);
	}

	#[test]
	fn handles_empty_and_malformed_input() {
		assert!(!Params::parse(None, None).contains("a"));
		assert!(!Params::parse(Some(""), Some(b"")).contains("a"));
		let params = Params::parse(Some("a&b=%ZZ&=x&c=%F0%9F%8E%A7"), None);
		assert_eq!(params.get("a"), Some(""));
		assert_eq!(params.get("c"), Some("🎧"));
	}
}
