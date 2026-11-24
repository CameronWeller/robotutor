use super::*;

pub(super) fn questions(author_path: &str) -> Result<Value, String> {
    let rows = commissioned(&receipt(author_path)?, false)?;
    let mut displays = serde_json::Map::new();
    for (id, row) in ["T1", "T3", "P1"].into_iter().zip(&rows) {
        let pack = row
            .batch
            .available(row.work, &row.pins)
            .map_err(|e| format!("{e:?}"))?;
        displays.insert(id.to_owned(), json!(pack.check));
    }
    Ok(json!({"questions":displays, "source": reviewed()["first_pack"]["excerpt"]}))
}

fn next_step(result: CheckResult) -> &'static str {
    match result {
        CheckResult::SupportsDurability => "You applied the balance to a different boundary. Would you like another water-budget problem or to choose your next topic?",
        CheckResult::OpposesDurability => "Let's review which movements cross this boundary before trying a fresh balance.",
        CheckResult::Ineligible => "Let's keep a changed-boundary check open for another visit.",
        _ => "Let's leave this check open until we can make a spaced comparison.",
    }
}

pub(super) fn score(author_path: &str, teaching_path: &str) -> Result<Value, String> {
    let [first, alias, mut fresh] = commissioned(&receipt(author_path)?, false)?;
    let logs = receipt(teaching_path)?;
    let answer = |id: &str, row: &Released| -> Result<&str, String> {
        let packet = &logs[id]["packet"];
        let expected = json!({"question":row.record["check"]});
        if packet != &expected {
            return Err(format!(
                "{id} reader packet differs from the admitted question"
            ));
        }
        logs[id]["response"]["display"]
            .as_str()
            .ok_or_else(|| format!("missing {id} submission"))
    };
    let first_text = answer("T1", &first)?;
    let alias_text = answer("T3", &alias)?;
    let fresh_text = answer("P1", &fresh)?;
    let tutor_packet = json!({"question": first.record["check"], "submission": first_text,
        "explanation":logs["T1"]["response"]["explanation"],
        "reviewed_principle":reviewed()["first_pack"]["excerpt"]});
    if logs["feedback"]["packet"] != tutor_packet
        || logs["feedback"]["response"]["display"]
            .as_str()
            .is_none_or(|s| s.trim().is_empty())
    {
        return Err("day-one feedback must follow the actual first submission".into());
    }
    let (a, first_case) =
        first.assess(visit(1, Assistance::NoneReported), first_text, &first.pins)?;
    let (a_guided, _) = first.assess(visit(1, Assistance::Reported), first_text, &first.pins)?;
    let (b, _) = alias.assess(visit(30, Assistance::NoneReported), alias_text, &alias.pins)?;
    let (c, changed_case) =
        fresh.assess(visit(30, Assistance::NoneReported), fresh_text, &fresh.pins)?;
    let (c_guided, _) = fresh.assess(visit(30, Assistance::Reported), fresh_text, &fresh.pins)?;
    let pairs = [(first_case, changed_case)].into();
    let withdrawn = Withdrawn::new(cell().scope);
    let measure = |prior: &Evidence, later: &Evidence, contacts: &[Exposure]| {
        delayed_on_reviewed_change(
            prior,
            later,
            Some(contacts),
            &withdrawn,
            Spacing {
                stability_days: 5.,
                threshold: 0.85,
                target_day: None,
            },
            &pairs,
        )
        .map_err(|e| format!("{e:?}"))
    };
    let feedback = Exposure {
        cell: cell(),
        at: Moment {
            day: 1,
            sequence: 2,
        },
    };
    let refresher = Exposure {
        cell: cell(),
        at: Moment {
            day: 29,
            sequence: 1,
        },
    };
    let result = |prior: &Evidence,
                  later: &Evidence,
                  contacts: &[Exposure]|
     -> Result<Value, String> {
        let measured = measure(prior, later, contacts)?;
        Ok(
            json!({"judgment":format!("{:?}",later.judgment()),"spacing":format!("{measured:?}"),
            "display":next_step(measured)}),
        )
    };
    let mut output = json!({"first_judgment":format!("{:?}",a.judgment()),
        "T3":result(&a,&b,&[feedback])?, "P1":result(&a,&c,&[feedback])?,
        "first_answer_after_help":result(&a_guided,&c,&[feedback])?,
        "return_answer_after_help":result(&a,&c_guided,&[feedback])?,
        "recent_refresher":result(&a,&c,&[feedback,refresher])?});
    let mut remaining = fresh.pins.clone();
    if !remaining.remove(&source_span(&reviewed()["first_pack"])) {
        return Err("reviewed USGS passage missing from required pins".into());
    }
    let display = fresh.batch.available(fresh.work, &remaining).err();
    let score = fresh
        .assess(visit(30, Assistance::NoneReported), fresh_text, &remaining)
        .err();
    fresh
        .batch
        .withdraw(
            fresh.work,
            fresh.receipt,
            &ReviewerGrant {
                actor: 2,
                work: fresh.work,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
    let restored = fresh
        .assess(visit(30, Assistance::NoneReported), fresh_text, &fresh.pins)
        .err();
    output["withdrawal"] = json!({"source_missing_display":display.map(|e|format!("{e:?}")),
        "source_missing_score":score,"source_restored_after_row_withdrawal":restored});
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_held_out_responses_change_teaching_under_current_grants() {
        let author = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/foundation/water-authoring.md"
        );
        let live = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/water-teaching.json");
        let results = score(author, live).unwrap();
        assert_eq!(results["T3"]["judgment"], "Correct");
        assert_eq!(results["T3"]["spacing"], "Ineligible");
        assert_eq!(results["P1"]["spacing"], "SupportsDurability");
        assert_ne!(results["T3"]["display"], results["P1"]["display"]);
        for label in ["first_answer_after_help", "return_answer_after_help"] {
            assert_eq!(results[label]["spacing"], "Ineligible");
        }
        assert_eq!(
            results["recent_refresher"]["spacing"],
            "InsufficientSpacing"
        );
        assert_eq!(
            results["withdrawal"]["source_missing_display"],
            "SourceUnavailable"
        );
        assert_eq!(
            results["withdrawal"]["source_missing_score"],
            "SourceUnavailable"
        );
        assert_eq!(
            results["withdrawal"]["source_restored_after_row_withdrawal"],
            "Withdrawn"
        );
    }
}
