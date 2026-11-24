//! A single native Jev choice. The caller defines the eligible actions and executes
//! the selected one. The provider sees the alternatives and the public evidence for them.
use super::{reported_model, typed_choice, valid_model};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub struct DecisionMenu {
    key: String,
    request: Value,
}

impl DecisionMenu {
    /// The contract identifies the meaning of this decision. Scope ties it to local
    /// inputs (for example a release or repair attempt) that need not leave the host.
    pub fn new(
        contract: &str,
        model: &str,
        scope: &Value,
        state: &Value,
        instructions: &str,
        criteria: Value,
    ) -> Result<Self, String> {
        let choices = criteria.as_object().ok_or("expected named Jev choices")?;
        if !valid_model(model) || contract.trim().is_empty() || instructions.trim().is_empty() {
            return Err("a Jev menu needs a contract, a model and selection instructions".into());
        }
        if !(state.is_string() || state.is_object() || state.is_array()) {
            return Err("a Jev decision needs text, an object or an array as its evidence".into());
        }
        if choices.len() < 2
            || choices.iter().any(|(label, criterion)| {
                label.trim().is_empty()
                    || criterion.as_str().is_none_or(|text| text.trim().is_empty())
            })
        {
            return Err("a Jev menu needs at least two described actions".into());
        }
        let question = json!({"type":"choice","instructions":instructions,"criteria":criteria});
        let mut digest = Sha256::new();
        serde_json::to_writer(&mut digest, &(contract, model, scope, state, &question))
            .map_err(|_| "could not bind the Jev menu")?;
        let digest = format!("{:x}", digest.finalize());
        let key = format!("choice_{}", &digest[..32]);
        let request = json!({"model":model,"state":state,"questions":{(&key):question}});
        Ok(Self { key, request })
    }

    pub fn request(&self) -> &Value {
        &self.request
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// Accept exactly one answer for this menu. The allowed set is the same object
    /// that went on the wire, including any eligibility restrictions for this turn.
    pub fn selected<'a>(&self, capture: &'a Value) -> Result<&'a str, String> {
        let model = self.request["model"].as_str().expect("constructed model");
        reported_model(capture, model)?;
        if capture["answers"]
            .as_object()
            .is_none_or(|answers| answers.len() != 1)
        {
            return Err("expected exactly one answer to the Jev menu".into());
        }
        let label = typed_choice(capture, &self.key)?;
        if self.request["questions"][&self.key]["criteria"]
            .get(label)
            .is_none()
        {
            return Err("Jev selected an action outside the current menu".into());
        }
        Ok(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openrouter::{DEFAULT_JEV, DEFAULT_LUNA};

    fn menu(scope: &str, state: Value, instructions: &str, criteria: Value) -> DecisionMenu {
        DecisionMenu::new(
            "opening-help/1",
            DEFAULT_JEV,
            &json!(scope),
            &state,
            instructions,
            criteria,
        )
        .unwrap()
    }

    #[test]
    fn replies_are_specific_to_the_exact_scope_evidence_and_definition_of_each_choice() {
        let scope = "unshared-release-id";
        let state = json!({"request":"Show me how."});
        let instructions = "Choose a released teaching aid.";
        let criteria =
            json!({"example":"Demonstrate the method.","source":"Read the original wording."});
        let first = menu(scope, state.clone(), instructions, criteria.clone());
        assert!(!first.request().to_string().contains(scope));
        let reply = json!({"model":DEFAULT_JEV,"answers":{(first.key()):{"type":"choice","choice":"example"}}});
        assert_eq!(first.selected(&reply).unwrap(), "example");
        for next in [
            menu(
                "other release",
                state.clone(),
                instructions,
                criteria.clone(),
            ),
            menu(
                scope,
                json!({"request":"Where is that written?"}),
                instructions,
                criteria.clone(),
            ),
            menu(
                scope,
                state.clone(),
                "Prefer the source if both fit.",
                criteria.clone(),
            ),
            menu(
                scope,
                state.clone(),
                instructions,
                json!({"example":"Show an unrelated example.","source":"Read the original wording."}),
            ),
        ] {
            assert!(next.selected(&reply).is_err());
        }
        assert!(DecisionMenu::new(
            "test",
            DEFAULT_JEV,
            &Value::Null,
            &state,
            instructions,
            json!({"only":"There is no choice to make."})
        )
        .is_err());
    }

    #[test]
    fn the_model_must_return_one_offered_action_and_cannot_grant_itself_authority() {
        let menu = menu(
            "fixture",
            json!({"draft":"An opening"}),
            "Choose a disposition.",
            json!({"hold":"Set aside for a reviewer.","revise":"Edit the task."}),
        );
        let valid = json!({"model":DEFAULT_JEV,"answers":{(menu.key()):{"type":"choice","choice":"revise"}}});
        let mut extra = valid.clone();
        extra["answers"]["publish"] = json!({"type":"choice","choice":"yes"});
        let mut unoffered = valid.clone();
        unoffered["answers"][menu.key()]["choice"] = json!("publish");
        let mut wrong_model = valid;
        wrong_model["model"] = json!(DEFAULT_LUNA);
        for capture in [extra, unoffered, wrong_model] {
            assert!(menu.selected(&capture).is_err());
        }
    }
}
