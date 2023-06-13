use crate::err::Error;
use crate::idx::ft::analyzer::tokenizer::{Tokenizer, Tokens};
use crate::idx::ft::doclength::DocLength;
use crate::idx::ft::offsets::{Offset, OffsetRecords};
use crate::idx::ft::postings::TermFrequency;
use crate::idx::ft::terms::{TermId, Terms};
use crate::kvs::Transaction;
use crate::sql::statements::DefineAnalyzerStatement;
use crate::sql::tokenizer::Tokenizer as SqlTokenizer;
use crate::sql::Array;
use filter::Filter;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

mod filter;
mod tokenizer;

pub(crate) struct Analyzers {}

impl Analyzers {
	pub(crate) const LIKE: &'static str = "like";
}

pub(super) struct Analyzer {
	t: Option<Vec<SqlTokenizer>>,
	f: Option<Vec<Filter>>,
}

impl From<DefineAnalyzerStatement> for Analyzer {
	fn from(az: DefineAnalyzerStatement) -> Self {
		Self {
			t: az.tokenizers,
			f: Filter::from(az.filters),
		}
	}
}

impl Analyzer {
	pub(super) async fn extract_terms(
		&self,
		t: &Terms,
		tx: &mut Transaction,
		query_string: String,
	) -> Result<(Vec<TermId>, bool), Error> {
		let tokens = self.analyse(query_string);
		// We first collect every unique terms
		// as it can contains duplicates
		let mut terms = HashSet::new();
		for token in tokens.list() {
			terms.insert(token);
		}
		let mut missing = false;
		// Now we can extract the term ids
		let mut res = Vec::with_capacity(terms.len());
		for term in terms {
			if let Some(term_id) = t.get_term_id(tx, tokens.get_token_string(term)).await? {
				res.push(term_id);
			} else {
				missing = false;
			}
		}
		Ok((res, missing))
	}

	/// This method is used for indexing.
	/// It will create new term ids for non already existing terms.
	pub(super) async fn extract_terms_with_frequencies(
		&self,
		terms: &mut Terms,
		tx: &mut Transaction,
		field_content: &Array,
	) -> Result<(DocLength, Vec<(TermId, TermFrequency)>), Error> {
		let mut dl = 0;
		// Let's first collect all the inputs, and collect the tokens.
		// We need to store them because everything after is zero-copy
		let inputs = self.analyse_content(field_content)?;
		// We then collect every unique terms and count the frequency
		let mut tf: HashMap<&str, TermFrequency> = HashMap::new();
		for tks in &inputs {
			for tk in tks.list() {
				dl += 1;
				let s = tks.get_token_string(tk);
				match tf.entry(s) {
					Entry::Vacant(e) => {
						e.insert(1);
					}
					Entry::Occupied(mut e) => {
						e.insert(*e.get() + 1);
					}
				}
			}
		}
		// Now we can resolve the term ids
		let mut tfid = Vec::with_capacity(tf.len());
		for (t, f) in tf {
			tfid.push((terms.resolve_term_id(tx, t).await?, f));
		}
		Ok((dl, tfid))
	}

	/// This method is used for indexing.
	/// It will create new term ids for non already existing terms.
	pub(super) async fn extract_terms_with_frequencies_with_offsets(
		&self,
		terms: &mut Terms,
		tx: &mut Transaction,
		field_content: &Array,
	) -> Result<(DocLength, Vec<(TermId, TermFrequency)>, Vec<(TermId, OffsetRecords)>), Error> {
		let mut dl = 0;
		// Let's first collect all the inputs, and collect the tokens.
		// We need to store them because everything after is zero-copy
		let inputs = self.analyse_content(field_content)?;
		// We then collect every unique terms and count the frequency and extract the offsets
		let mut tfos: HashMap<&str, Vec<Offset>> = HashMap::new();
		for (i, tks) in inputs.iter().enumerate() {
			for tk in tks.list() {
				dl += 1;
				let s = tks.get_token_string(tk);
				let o = tk.new_offset(i as u32);
				match tfos.entry(s) {
					Entry::Vacant(e) => {
						e.insert(vec![o]);
					}
					Entry::Occupied(mut e) => e.get_mut().push(o),
				}
			}
		}

		// Now we can resolve the term ids
		let mut tfid = Vec::with_capacity(tfos.len());
		let mut osid = Vec::with_capacity(tfos.len());
		for (t, o) in tfos {
			let id = terms.resolve_term_id(tx, t).await?;
			tfid.push((id, o.len() as TermFrequency));
			osid.push((id, OffsetRecords(o)));
		}
		Ok((dl, tfid, osid))
	}

	fn analyse_content(&self, field_content: &Array) -> Result<Vec<Tokens>, Error> {
		let mut res = Vec::with_capacity(field_content.0.len());
		for v in &field_content.0 {
			let input = v.to_owned().convert_to_string()?;
			let tks = self.analyse(input);
			res.push(tks);
		}
		Ok(res)
	}

	fn analyse(&self, input: String) -> Tokens {
		if let Some(t) = &self.t {
			if !input.is_empty() {
				let t = Tokenizer::tokenize(t, input);
				return Filter::apply_filters(t, &self.f);
			}
		}
		Tokens::new(input)
	}
}

#[cfg(test)]
mod tests {
	use super::Analyzer;
	use crate::sql::statements::define::analyzer;

	pub(super) fn test_analyser(def: &str, input: &str, expected: &[&str]) {
		let (_, az) = analyzer(def).unwrap();
		let a: Analyzer = az.into();

		let tokens = a.analyse(input.to_string());
		let mut res = vec![];
		for t in tokens.list() {
			res.push(tokens.get_token_string(t));
		}
		assert_eq!(&res, expected);
	}
}