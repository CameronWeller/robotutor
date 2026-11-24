//! A few synthetic editorial submissions for a *different* exercise on Psalm 100.
use crate::curriculum::{
    creation::{source_manifest, Batch, Candidate, Error, Pack, ReviewerGrant, Work},
    OutcomeId, Span,
};
use crate::material::document_id;
use crate::supply::Contact;
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};

const DISTINCT:&str="A fictional choir loses its outdoor rehearsal to rain. A neighbor opens a dry hall for them. Give an invented singer a line of thanksgiving grounded in something already received, and connect that choice to the verse. You may instead point to words in the verse or keep your response private.";
// Frozen independently of the candidate builder. An altered case needs a fresh
// editorial receipt even if its label and source remain unchanged.
const REVIEWED_DIGEST: &str = "fa17fb87324a9b34b51ff4b4dc64c85591291b44a7c33197f3f6d2cd3e149a7e";
const AUTHOR: u64 = 10;
const EDITOR: u64 = 11;

fn draft(name: &str) -> Result<(&'static str, &'static str), String> {
    Ok(match name{
        "paraphrase"=>("A night-shift worker walks home by a lamp a neighbor left burning. Write the worker's thanks and connect it to the verse, or pass.","The lamp, journey and worked response are the prior night-worker task."),
        "disguised"=>("At dawn, an invented baker follows the lantern a friend set beside the path. Give the traveler a line of thanks and name the source phrase it echoes, or pass.","The same guiding light on the journey supplies the same particular reason and response despite new nouns."),
        "personal"=>("Tell the group how sincerely you personally worshipped God yesterday. Quote the verse and describe your actual prayer.","This asks for personal devotion, group disclosure and a sincerity judgment outside the objective."),
        "hall"=>(DISTINCT,"An apparent setback and a received gift must be distinguished. The singer and scene are invented, the source supports thanksgiving, and the response may stay private."),
        _=>return Err("editor fixture must be paraphrase, disguised, personal or hall".into()),
    })
}

struct Publication {
    batch: Batch,
    work: Work,
}

#[derive(Clone, Default)]
struct Commission {
    requested: bool,
    reviewed: Option<Arc<Publication>>,
}

impl Commission {
    /// The scope of this commission is the one reviewed hall exercise in this catalog.
    fn request(&mut self) -> bool {
        !std::mem::replace(&mut self.requested, true)
    }

    fn stage(pack: &Pack, name: &str) -> Result<(Batch, Work, [u8; 32]), String> {
        let (text, _) = draft(name)?;
        let mut candidate = pack.clone();
        candidate.check = text.into();
        let work=Work{release:1,objective:OutcomeId(101),brief:document_id(b"Psalm 100 supplied text; different fictional circumstance; preserve privacy and criterion"),source_manifest:source_manifest(&pack.sources)};
        let mut batch = Batch::new(1, [work]).map_err(|e| format!("{e:?}"))?;
        let digest = batch
            .stage(vec![Candidate {
                work,
                author: AUTHOR,
                pack: candidate,
            }])
            .map_err(|e| format!("{e:?}"))?
            .remove(0)
            .map_err(|e| format!("{e:?}"))?;
        Ok((batch, work, digest))
    }
    pub fn review(&mut self, pack: &Pack, name: &str) -> Result<Value, String> {
        if !self.requested {
            return Err("no additional case requested".into());
        }
        let (_, rationale) = draft(name)?;
        let (mut batch, work, hash) = Self::stage(pack, name)?;
        let unavailable = batch.available(work, &pack.sources) == Err(Error::Unapproved);
        let self_signed = batch.approve(
            work,
            hash,
            &ReviewerGrant {
                actor: AUTHOR,
                work,
            },
            &pack.sources,
        );
        if !unavailable || self_signed != Err(Error::UnauthorizedReviewer) {
            return Err("unexpected authoring gate".into());
        }
        let digest = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let approved = name == "hall" && digest == REVIEWED_DIGEST;
        if approved {
            batch
                .approve(
                    work,
                    hash,
                    &ReviewerGrant {
                        actor: EDITOR,
                        work,
                    },
                    &pack.sources,
                )
                .map_err(|e| format!("{e:?}"))?;
            batch
                .available(work, &pack.sources)
                .map_err(|e| format!("{e:?}"))?;
            self.reviewed = Some(Arc::new(Publication { batch, work }));
        }
        Ok(
            json!({"contribution":"editorial_fixture","candidate":name,"pack_digest":digest,"structurally_staged":true,"held_before_review":unavailable,
            "self_approval_refused":true,"editor_disposition":if approved{"approved"}else{"withheld"},"editor_reason":rationale,"display":null,"credit":null}),
        )
    }
    fn check<'a>(&'a self, pack: &Pack, permitted: &BTreeSet<Span>) -> Result<&'a str, String> {
        let publication = self.reviewed.as_ref().ok_or("unreviewed exercise")?;
        let approved = publication
            .batch
            .available(publication.work, permitted)
            .map_err(|e| format!("{e:?}"))?;
        // The previous commissioning path replaced only the original check.
        // Retain its full-pack comparison without staging or signing again on read.
        let mut candidate = pack.clone();
        candidate.check.clone_from(&approved.check);
        if &candidate != approved {
            return Err("previous approval binds different pack contents".into());
        }
        Ok(&approved.check)
    }
}

/// Contact with the reviewed hall task. Confirmed absence requires a complete history.
#[derive(Clone, Default)]
struct HallContact(Option<Contact>);

impl HallContact {
    fn encountered(&mut self, level: Contact) {
        if self.0 != Some(Contact::Delivered) {
            self.0 = Some(level);
        }
    }

    fn fresh(&self, complete: bool) -> bool {
        complete && self.0.is_none()
    }

    fn open(
        &mut self,
        commission: &Commission,
        pack: &Pack,
        permitted: &BTreeSet<Span>,
        complete: bool,
    ) -> Result<String, String> {
        if !self.fresh(complete) || commission.reviewed.is_none() {
            return Err("no unseen authorized extra case".into());
        }
        let check = commission.check(pack, permitted)?;
        // The caller holds this learner's contact record through publishing the bytes.
        self.encountered(Contact::Possible);
        Ok(check.to_owned())
    }
}

#[derive(Clone, Default)]
pub(super) struct Supply {
    commission: Commission,
    contact: HallContact,
}

impl Supply {
    pub fn available(&self) -> bool {
        self.commission.reviewed.is_some() && self.contact.fresh(true)
    }
    pub fn contact(&self) -> Option<Contact> {
        self.contact.0
    }
    pub fn report_familiar(&mut self) -> Result<(), String> {
        if !self.available() {
            return Err("no different released case is available".into());
        }
        self.contact.encountered(Contact::Possible);
        Ok(())
    }
    pub fn confirm(&mut self) -> Result<(), String> {
        if self.contact.0.is_none() {
            return Err("no reserved additional case".into());
        }
        self.contact.encountered(Contact::Delivered);
        Ok(())
    }
    pub fn request(&mut self) -> Value {
        self.commission.request();
        json!({"contribution":"case_requested","display":"Another fictional use of the same verse has been requested. While it is reviewed, you can revisit the example, try guided practice, or stop.","credit":null})
    }
    pub fn review(&mut self, pack: &Pack, name: &str) -> Result<Value, String> {
        self.commission.review(pack, name)
    }
    pub fn open(&mut self, pack: &Pack) -> Result<String, String> {
        self.contact
            .open(&self.commission, pack, &pack.sources, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rehearsal::goal_choice::published_psalm_lesson;
    #[test]
    fn an_edited_criterion_cannot_inherit_a_signed_case() {
        let original = published_psalm_lesson().unwrap();
        let pack = original.material(true).unwrap();
        let mut supply = Supply::default();
        supply.request();
        assert_eq!(
            supply.review(pack, "hall").unwrap()["editor_disposition"],
            "approved"
        );
        let mut changed = pack.clone();
        changed.criterion = "Disclose a sincere personal prayer.".into();
        assert!(supply.open(&changed).is_err());
        assert!(supply.contact.fresh(true));
        let mut new_review = Supply::default();
        new_review.request();
        assert_eq!(
            new_review.review(&changed, "hall").unwrap()["editor_disposition"],
            "withheld"
        );
        assert!(supply.open(pack).unwrap().contains("dry hall"));
        assert!(supply.open(pack).is_err());
    }

    #[test]
    fn one_hall_commission_preserves_per_class_rights_and_each_learner_contact() {
        use crate::audience::Roster;
        use std::collections::BTreeMap;

        struct Room {
            roster: Roster,
            // The host supplies current source rights and explicit private requests.
            spans: BTreeSet<Span>,
            requests: BTreeSet<u64>,
        }
        impl Room {
            fn ask(
                &mut self,
                actor: u64,
                commission: &mut Commission,
            ) -> Result<bool, &'static str> {
                if !self.roster.learners.contains(&actor) {
                    return Err("not enrolled");
                }
                self.requests.insert(actor);
                Ok(commission.request())
            }

            fn project(
                &self,
                actor: u64,
                audience: &[(u64, Contact)],
                commission: &Commission,
                pack: &Pack,
                contacts: &mut BTreeMap<u64, HallContact>,
            ) -> Result<String, String> {
                if !self.roster.teachers.contains(&actor)
                    || audience
                        .iter()
                        .any(|(l, _)| !self.roster.learners.contains(l))
                {
                    return Err("wrong projector or audience".into());
                }
                let prompt = commission.check(pack, &self.spans)?;
                // The audience is resolved at display time. Editing membership later does not
                // rewrite this delivery. Possible contact survives an uncertain projection.
                for (seat, level) in audience {
                    contacts.entry(*seat).or_default().encountered(*level);
                }
                Ok(prompt.to_owned())
            }

            fn fresh(
                &mut self,
                actor: u64,
                owner: u64,
                complete: bool,
                commission: &Commission,
                pack: &Pack,
                contacts: &mut BTreeMap<u64, HallContact>,
            ) -> Result<String, String> {
                if actor != owner || !self.roster.learners.contains(&owner) {
                    return Err("wrong learner".into());
                }
                if !self.requests.contains(&owner) {
                    return Err("no private request".into());
                }
                let result = contacts.entry(owner).or_default().open(
                    commission,
                    pack,
                    &self.spans,
                    complete,
                )?;
                self.requests.remove(&owner);
                Ok(result)
            }
        }

        let release = published_psalm_lesson().unwrap();
        let pack = release.material(true).unwrap();
        let mut first = Room {
            roster: Roster {
                class: 7,
                teachers: [71].into(),
                learners: [11, 12, 13, 14, 15, 16].into(),
            },
            spans: pack.sources.clone(),
            requests: BTreeSet::new(),
        };
        let mut second = Room {
            roster: Roster {
                class: 9,
                teachers: [91].into(),
                learners: [21, 22].into(),
            },
            spans: BTreeSet::new(),
            requests: BTreeSet::new(),
        };
        let mut commission = Commission::default();
        let mut jobs = 0;
        // 14 and 22 want to keep their work private. They request no independent question.
        for s in [11, 12, 13, 15, 16] {
            jobs += usize::from(first.ask(s, &mut commission).unwrap());
        }
        jobs += usize::from(second.ask(21, &mut commission).unwrap());
        assert_eq!(jobs, 1);
        let separate_jobs: usize = (0..first.requests.len() + second.requests.len())
            .map(|_| {
                let mut per_learner = Commission::default();
                assert!(per_learner.request());
                usize::from(
                    per_learner.review(pack, "hall").unwrap()["editor_disposition"] == "approved",
                )
            })
            .sum();
        assert_eq!(separate_jobs, 6);

        assert!(second.ask(11, &mut commission).is_err());
        let mut contacts = BTreeMap::new();
        let audience = [
            (11, Contact::Delivered),
            (12, Contact::Possible),
            (14, Contact::Delivered),
        ];
        assert!(first
            .project(71, &audience, &commission, pack, &mut contacts)
            .is_err());
        let reviewed = commission.review(pack, "hall").unwrap();
        assert_eq!(reviewed["editor_disposition"], "approved");
        assert!(first
            .project(91, &audience, &commission, pack, &mut contacts)
            .is_err());
        assert!(first
            .project(
                71,
                &[(21, Contact::Delivered)],
                &commission,
                pack,
                &mut contacts
            )
            .is_err());
        assert!(contacts.is_empty());
        let shared_prompt = first
            .project(71, &audience, &commission, pack, &mut contacts)
            .unwrap();
        assert!(shared_prompt.contains("dry hall"));
        assert!(shared_prompt.contains("keep your response private"));
        // Delivering a public example does not submit the attendee's private writing.
        assert!(first
            .fresh(14, 14, true, &commission, pack, &mut contacts)
            .is_err());
        assert!(first
            .fresh(11, 13, true, &commission, pack, &mut contacts)
            .is_err());
        let mut fresh = Vec::new();
        for (seat, complete) in [(11, true), (12, true), (13, true), (15, false), (16, true)] {
            if first
                .fresh(seat, seat, complete, &commission, pack, &mut contacts)
                .is_ok()
            {
                fresh.push(seat);
            }
        }
        assert_eq!(fresh, [13, 16]);
        // A catalog-wide "shown" flag would erase both absent learners' opportunities.
        let mut class_wide_contact = HallContact::default();
        class_wide_contact.encountered(Contact::Delivered);
        assert!(fresh.iter().all(|_| class_wide_contact
            .open(&commission, pack, &first.spans, true)
            .is_err()));
        assert!(first
            .fresh(13, 13, true, &commission, pack, &mut contacts)
            .is_err());
        assert!(second
            .project(
                91,
                &[(21, Contact::Possible)],
                &commission,
                pack,
                &mut contacts
            )
            .is_err());
        assert!(second
            .fresh(21, 21, true, &commission, pack, &mut contacts)
            .is_err());
        assert!(contacts.get(&21).is_none_or(|c| c.0.is_none()));
        second.spans = pack.sources.clone();
        assert_eq!(
            second
                .fresh(21, 21, true, &commission, pack, &mut contacts)
                .unwrap(),
            shared_prompt
        );
        assert!(second
            .fresh(22, 22, true, &commission, pack, &mut contacts)
            .is_err());
        println!("acquisition requests=6, commissions={jobs}, successful editorial reviews=1; projection A definite=2 possible=1; independent openings A={fresh:?} B while held=0 B after restored=1; assessed responses=0");

        // Same installation, retained contact permission, stable learner principal.
        // 12 was only possibly present in the first class. Neither absence from the
        // new class history nor a new class grant suffices for a fresh check.
        first.roster.learners.remove(&12);
        second.roster.learners.insert(12);
        assert_eq!(
            second
                .fresh(12, 12, true, &commission, pack, &mut contacts)
                .unwrap_err(),
            "no private request"
        );
        assert!(!second.ask(12, &mut commission).unwrap());
        let mut class_only_contacts = BTreeMap::new();
        assert_eq!(
            second
                .fresh(12, 12, true, &commission, pack, &mut class_only_contacts)
                .unwrap(),
            shared_prompt
        );
        // Restore the learner's explicit request to replay the alternative policy.
        second.ask(12, &mut commission).unwrap();
        assert!(second
            .fresh(12, 12, true, &commission, pack, &mut contacts)
            .is_err());
        println!("transferred possible contact, new explicit request: class-only history reopened the same case; retained task contact withheld freshness");
    }
}

#[cfg(test)]
mod revision_evidence;
