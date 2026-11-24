"""Audit the five-lane synthetic policy campaign and summarize the observed decisions.

    python lab/studies/revision_report.py RUN_ROOT OUTPUT_DIR --target-per-domain 30

Writes summary.json, editable summary.md and evidence.zip. The archive contains the
public synthetic round records, selected stage receipts and a SHA-256 manifest. Runtime
streams, environment, transport error text and unrecognized files do not enter it.
Use an operator-reviewed synthetic run. A local hash establishes content identity,
not provider authentication. Run again after the writer finishes to include late calls.
"""
import argparse
from collections import Counter
from contextlib import redirect_stdout
import hashlib
import html
import io
import json
import math
from pathlib import Path
import re
import stat
import tempfile
import zipfile
from unittest.mock import patch

import design_revisions as runner
from teaching_report import safe_bytes


STAGES = ("case", "design", "review", *(arm + "-" + stage for arm in ("A", "B") for stage in
          ("first", "learner", "feedback", "return", "return-learner", "comparison")))
TOKENS = ("input_tokens", "cached_input_tokens", "output_tokens", "reasoning_output_tokens")
HASH = re.compile(r"[a-f0-9]{64}")
SECRET = re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----|\b(?:sk-(?:proj-)?[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}|Bearer\s+[A-Za-z0-9._=-]{16,})")


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False) + "\n").encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def directory(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or getattr(info, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT:
        raise ValueError("unexpected directory type")


def load(path):
    data = safe_bytes(path)
    value = json.loads(data)
    if not isinstance(value, dict):
        raise ValueError("expected object")
    return data, value


def package_member(files, name, data):
    if SECRET.search(data.decode()):
        raise ValueError("possible credential")
    files[name] = data


def receipt_fields(value):
    seconds = value["seconds"]
    if type(seconds) not in (float, int) or not math.isfinite(seconds) or seconds < 0:
        raise ValueError("invalid elapsed time")
    if not HASH.fullmatch(value["request_hash"]) or not isinstance(value["model_requested"], str):
        raise ValueError("invalid request identity")
    usage = value["tokens"]
    if usage is not None and (not isinstance(usage, dict) or
                              any(type(n) is not int or n < 0 for n in usage.values())):
        raise ValueError("invalid token usage")
    return {k: value[k] for k in ("request_hash", "model_requested", "seconds", "tokens")}


def meter(totals, usage, seconds):
    totals["reported_call_seconds"] += seconds
    for key in TOKENS:
        if key in (usage or {}):
            totals[key] += usage[key]
            totals[key + "_receipts"] += 1


def role_name(stage):
    if stage in ("case", "design", "review"):
        return stage
    phase = stage.split("-", 1)[1]
    if phase in ("first", "return"):
        return "teaching coach"
    if phase == "feedback":
        return "teaching feedback"
    if phase == "comparison":
        return "comparison participant"
    return "teaching participant"


def role_costs(rows):
    """Nearest-rank percentiles of accepted CLI calls. Failed attempts have separate totals."""
    roles = {}
    for stage, status, usage, elapsed in rows:
        group = roles.setdefault(role_name(stage), {"accepted": Counter(), "failed": Counter(), "latencies": []})
        group[status]["calls"] += 1
        meter(group[status], usage, elapsed)
        if status == "accepted":
            group["latencies"].append(elapsed)
    for role, group in roles.items():
        ordered = sorted(group.pop("latencies"))
        group["purpose"] = "tutor delivery" if role in ("teaching coach", "teaching feedback") else "study only"
        for q in (50, 95):
            group[f"p{q}_accepted_seconds"] = ordered[math.ceil(len(ordered) * q / 100) - 1] if ordered else None
    return roles


def atomic(path, data):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        stream.write(data)
        temp = Path(stream.name)
    try:
        temp.replace(path)
    finally:
        temp.unlink(missing_ok=True)


def archive(files):
    # Stable timestamps and order let operators compare repeated reports byte for byte.
    manifest = "".join(f"{digest(data)}  {path}\n" for path, data in sorted(files.items()))
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as bundle:
        for path, data in sorted({**files, "SHA256SUMS": manifest.encode()}.items()):
            member = zipfile.ZipInfo(path)
            member.compress_type = zipfile.ZIP_DEFLATED
            bundle.writestr(member, data)
    return output.getvalue()


def read_round(folder, metrics, files, calls):
    """Count observed calls including incomplete rounds. Only replayed full receipts ship."""
    records = {}
    errors = []
    prefix = folder.parent.name + "/" + folder.name + "/"
    for stage in STAGES:
        path = folder / (stage + ".json")
        if path.exists():
            try:
                raw, result = load(path)
                meta = receipt_fields(result)
                if set(result) != set(meta) | {"instruction", "packet", "response"}:
                    raise ValueError("unexpected fields")
                if meta["model_requested"] != runner.MODEL:
                    raise ValueError("unexpected model")
                records[stage] = (raw, result)
                metrics["accepted_receipts"] += 1
                meter(metrics, meta["tokens"], meta["seconds"])
                calls.append((stage, "accepted", meta["tokens"], meta["seconds"]))
                # Partial checkpoints have independently usable usage telemetry.
                meta["source_sha256"] = digest(raw)
                package_member(files, prefix + stage + "-usage.json", encoded(meta))
            except (KeyError, OSError, ValueError, TypeError):
                errors.append(f"invalid {stage} receipt")
        failure = folder / (stage + "-failures.jsonl")
        if failure.exists():
            attempts = []
            try:
                for line in safe_bytes(failure).splitlines():
                    row = json.loads(line)
                    meta = receipt_fields({**row, "model_requested": runner.MODEL})
                    if row.get("reason") == "revise at least one executable route, material or memory decision":
                        meta["category"] = "no_executable_policy_change"
                    elif row.get("reason") == "quote must be an exact substring of that arm's witness stage":
                        meta["category"] = "quote_not_in_registered_stage"
                    else:
                        meta["category"] = "other_recorded_failure"
                    metrics["failed_attempts"] += 1
                    metrics[meta["category"]] += 1
                    meter(metrics, meta["tokens"], meta["seconds"])
                    calls.append((stage, "failed", meta["tokens"], meta["seconds"]))
                    attempts.append(meta)
            except (OSError, TypeError, KeyError, ValueError):
                errors.append(f"incomplete {stage} failure ledger")
            if attempts:
                package_member(files, prefix + stage + "-failed-usage.json", encoded(attempts))
    return records, errors


def replay(folder, records, course, previous, mechanisms, parent):
    """Re-run the committed controller with a receipt-only substitute for the model."""
    raw, round_record = load(folder / "round.json")
    schema = round_record.get("schema")
    if type(schema) is not int or schema not in (1, 2, 3):
        raise ValueError("unknown campaign protocol")
    prereg = None
    if schema >= 2:
        prereg, _ = load(folder / "preregistration.json")
        if digest(prereg) != round_record["preregistration"]:
            raise ValueError("preregistration hash mismatch")
    domain = folder.parent.name
    ordinal = int(folder.name)
    used = set()
    original = runner.live
    with tempfile.TemporaryDirectory(prefix="rt-evidence-read-") as name:
        lane = Path(name)
        scratch = lane / folder.name
        scratch.mkdir()
        if prereg is not None:
            (scratch / "preregistration.json").write_bytes(prereg)
        for stage, (data, _) in records.items():
            (scratch / (stage + ".json")).write_bytes(data)

        def offline(directory, stage, instruction, packet, schema, check=lambda value: None):
            # Only this private scratch copy is writable. The runner's existing-checkpoint
            # branch validates the actual requested model+prompt hash and response schema.
            if stage not in records or not (directory / (stage + ".json")).is_file():
                raise ValueError(f"missing {stage} receipt")
            recorded = records[stage][1]
            if recorded["packet"] != packet or recorded["instruction"] != instruction:
                raise ValueError(f"{stage} packet mismatch")
            used.add(stage)
            return original(directory, stage, instruction, packet, schema, check)

        with patch.object(runner, "live", offline), redirect_stdout(io.StringIO()):
            rebuilt = runner.one_round(lane, domain, ordinal, course, previous, mechanisms, parent, schema=schema)
    if rebuilt != round_record or set(records) != used:
        raise ValueError("round differs from receipt replay")
    return raw, rebuilt, prereg


def round_summary(record, digest_value, receipts, target):
    review = record["review"]
    domain, n = record["domain"], record["ordinal"]
    candidate = next(a for a in record["arms"] if a["name"] == record["candidate_arm"])
    incumbent = next(a for a in record["arms"] if a["name"] != record["candidate_arm"])
    visible = ["first_display", "learner_reply", "feedback_display", "return_display", "return_reply"]
    comparison = "comparison_reply" in candidate and "comparison_reply" in incumbent
    if comparison:
        visible.append("comparison_reply")
    witness = record["proposal"].get("witness")
    coded = None
    if witness:
        coded = {
            **witness,
            "candidate": review[candidate["name"] + "_witness"],
            "incumbent": review[incumbent["name"] + "_witness"],
            "result": record["witness_result"],
        }
    routing = ("first_route", "return_route", "first_material", "return_material", "memory")
    fingerprint, bars = runner.completion_evidence(record)
    return {
        "id": f"{domain}/{n:03d}", "sha256": digest_value, "parent": record["parent"],
        "protocol": record["schema"], "common_comparison": comparison, "witness": coded,
        "setting": receipts["case"][1]["packet"]["setting"],
        "completion_fingerprint": fingerprint, "completion_exclusions": bars,
        "ordinal": n, "contingency": n > target, "days_deferred": record["case"]["days"],
        "candidate_arm": candidate["name"], "policy_delta": [key for key in candidate["policy"]
            if candidate["policy"][key] != incumbent["policy"][key]],
        "candidate_policy_sha256": digest(encoded(candidate["policy"])),
        "incumbent_policy_sha256": digest(encoded(incumbent["policy"])),
        "control_delta_sha256": digest(encoded([[k, incumbent["policy"][k], candidate["policy"][k]]
                                                for k in routing if incumbent["policy"][k] != candidate["policy"][k]])),
        "question": record["proposal"]["question"], "prediction": record["proposal"]["prediction"],
        "mechanism": review["mechanism"], "substantive": review["substantive"],
        "observed": review["observed"], "preferred": review["prefer"],
        "disposition": record["disposition"], "finding": review["finding"],
        "next_question": review["next_question"], "accepted_receipts": len(receipts),
        "candidate_routes": [candidate["policy"][key + "_route"] for key in ("first", "return")],
        "incumbent_routes": [incumbent["policy"][key + "_route"] for key in ("first", "return")],
        "candidate_memory_used": bool(candidate["memory"]), "memory_allowed": record["case"]["allow_memory"],
        "return_work_differs": candidate["return_reply"] != incumbent["return_reply"],
        "first_display_differs": candidate["first_display"] != incumbent["first_display"],
        "return_display_differs": candidate["return_display"] != incumbent["return_display"],
        "comparison_display": candidate["comparison_packet"]["display"] if comparison else None,
        "verbatim": {label: {key: arm[key] for key in visible} for label, arm in
                     (("candidate", candidate), ("incumbent", incumbent))},
    }


def scan(root, target):
    directory(root)
    files, lanes, issues = {}, {}, []
    totals, calls = Counter(), []
    for domain in runner.DOMAINS:
        lane = root / domain
        rows, metrics, partial = [], Counter(), []
        lanes[domain] = {"rounds": rows, "usage": metrics, "pending": partial}
        if not lane.exists():
            continue
        directory(lane)
        previous, parent, recent = None, "genesis", []
        course = runner.pack(domain)
        for folder in sorted((p for p in lane.iterdir() if re.fullmatch(r"\d{3,4}", p.name)),
                             key=lambda p: int(p.name)):
            if folder.name != f"{int(folder.name):03d}":
                issues.append(f"{domain}/{folder.name} has a noncanonical ordinal")
                continue
            directory(folder)
            # A live writer may finalize the round while its receipt files are being
            # scanned. Treat that folder as pending until the next snapshot.
            has_decision = (folder / "round.json").is_file()
            receipts, errors = read_round(folder, metrics, files, calls)
            name = domain + "/" + folder.name
            if not has_decision:
                partial.append({"id": name, "observed_stages": sorted(receipts)})
            elif not errors:
                try:
                    if int(folder.name) != len(rows) + 1:
                        raise ValueError("missing audited predecessor")
                    raw, record, prereg = replay(folder, receipts, course, previous, recent, parent)
                    members = {}
                    package_member(members, name + "/round.json", raw)
                    if prereg is not None:
                        package_member(members, name + "/preregistration.json", prereg)
                    for stage, (data, _) in receipts.items():
                        package_member(members, name + "/" + stage + ".json", data)
                    files.update(members)
                    parent = digest(raw)
                    previous = record
                    recent.append(record["review"]["mechanism"])
                    rows.append(round_summary(record, parent, receipts, target))
                except (ValueError, KeyError, TypeError, OSError):
                    errors.append("decision, packet, membership or lineage audit failed")
            issues.extend(name + " " + e for e in errors)
        totals.update(metrics)
    return lanes, dict(totals), issues, files, role_costs(calls)


def tally(rows):
    coded = [r for r in rows if r["witness"]]
    return {
        "executed": len(rows),
        "operationally_complete": sum(not r["completion_exclusions"] for r in rows),
        "unique_operationally_complete": sum(not r["completion_exclusions"] and
            r.get("previous_eligible_fingerprint") is None for r in rows),
        "completion_exclusions": dict(Counter(bar for r in rows for bar in r["completion_exclusions"])),
        "unique_policy_pairs": len({(r["incumbent_policy_sha256"], r["candidate_policy_sha256"]) for r in rows}),
        "unique_route_material_memory_changes": len({r["control_delta_sha256"] for r in rows}),
        "first_displays_differed": sum(r["first_display_differs"] for r in rows),
        "return_displays_differed": sum(r["return_display_differs"] for r in rows),
        "shared_comparison_observed": sum(r["common_comparison"] for r in rows),
        "witnesses": len(coded),
        "witness_results": dict(Counter(r["witness"]["result"] for r in coded)),
        "witness_stages": dict(Counter(r["witness"]["stage"] for r in coded)),
        "witness_review_disagreements": sum(r["observed"] != r["witness"]["result"] for r in coded),
        "repeated_witness_definitions_in_lane": sum(r.get("previous_identical_witness") is not None for r in coded),
        "reviewer_non_substantive": sum(not r["substantive"] for r in rows),
        "instruction_routes": dict(Counter(route for r in rows for k in ("incumbent_routes", "candidate_routes")
                                               for route in r[k])),
        "substantive": sum(r["substantive"] for r in rows),
        "adopted": sum(r["disposition"] == "adopt" for r in rows),
        "held": sum(r["disposition"] == "hold" for r in rows),
        "observations": dict(Counter(r["observed"] for r in rows)),
        "preferences": dict(Counter(r["preferred"] for r in rows)),
    }


def summary(lanes, usage, issues, target, roles):
    rows = [r for lane in lanes.values() for r in lane["rounds"]]
    for lane in lanes.values():
        seen, eligible = {}, {}
        for row in lane["rounds"]:
            witness = row["witness"]
            if witness:
                key = (witness["stage"], witness["difference"], witness["direction"])
                row["previous_identical_witness"] = seen.get(key)
                seen.setdefault(key, row["id"])
            if not row["completion_exclusions"]:
                key = row["completion_fingerprint"]
                row["previous_eligible_fingerprint"] = eligible.get(key)
                eligible.setdefault(key, row["id"])
        ordinary = [r for r in lane["rounds"] if not r["contingency"]]
        extra = [r for r in lane["rounds"] if r["contingency"]]
        done = {r["ordinal"] for r in ordinary}
        lane.update(planned=tally(ordinary), contingency=tally(extra),
                    missing_planned=[n for n in range(1, target + 1) if n not in done])
    lab = Path(__file__).resolve().parents[1]
    # Git-normalized text digests identify the implementation and releases a later
    # operator needs even if the checkout performs Windows newline conversion.
    sources = ["studies/design_revisions.py", "studies/teaching_trial.py", "studies/revision_report.py",
               "fixtures/revision-comparisons.json", "fixtures/goals/reviews.json",
               *(f"fixtures/goals/{d}.json" for d in runner.DOMAINS)]
    return {
        "schema": 1, "target_per_lane": target,
        "source_sha256_lf": {name: digest((lab / name).read_bytes().replace(b"\r\n", b"\n")) for name in sources},
        "planned": tally([r for r in rows if not r["contingency"]]),
        "contingency": tally([r for r in rows if r["contingency"]]),
        "all_decisions": tally(rows),
        "usage": usage, "roles": roles, "audit_issues": issues, "lanes": lanes,
        "by_protocol": {str(p): tally([r for r in rows if r["protocol"] == p])
                        for p in sorted({r["protocol"] for r in rows})},
        "by_setting": {s: tally([r for r in rows if r["setting"] == s])
                       for s in sorted({r["setting"] for r in rows})},
        "limits": ["Completed means both policy paths and their recorded decision reproduced from stage receipts.",
                   "Substantive is the reviewer model's per-pair judgment. Cross-lane mechanism novelty needs independent review.",
                   "Model usage comes from local CLI telemetry. Charges, full-run wall time, realized learning and competitor results are unmeasured."],
    }


def quoted(text, length=260):
    # Preserve the exact substring selected. Prefix each line as a Markdown quote.
    excerpt = text[:length]
    if len(text) > length and " " in excerpt:
        excerpt = excerpt.rsplit(" ", 1)[0]
    return "\n".join("> " + html.escape(line, quote=False) for line in excerpt.splitlines())


def markdown(report):
    target = report["target_per_lane"]
    planned, extra = report["planned"], report["contingency"]
    all_rows = report["all_decisions"]
    lanes = report["lanes"]
    lines = [
        "# Teaching policy decisions across five study lanes", "",
        f"The controller reproduced **{planned['executed']} of {target * len(lanes)} planned comparisons**. "
        f"The model reviewer called {planned['substantive']} of those instructional differences substantive. "
        f"Another {extra['executed']} comparisons ran beyond ordinal {target:03d} in their lanes, "
        f"of which the reviewer marked {extra['substantive']} substantive.", "",
        f"Across planned and contingency ordinals, **{all_rows['operationally_complete']}/{all_rows['executed']} "
        f"pairs met the operational completion rule**. **{all_rows['unique_operationally_complete']}** remained "
        "after retaining the first eligible instance of each control change per domain. This is a system-design "
        "evidence count. No human learning outcome was measured.", "",
        f"Within the planned comparisons, {planned['adopted']} candidates were adopted for the next round "
        f"and {planned['held']} incumbent policies were retained. A hold can follow a refutation, "
        "an unobserved prediction, a tie or an incumbent preference. A recorded adoption changes only that lane.", "",
        f"The {planned['executed']} replayed planned pairs included {planned['unique_policy_pairs']} distinct "
        f"full policy pairs and {planned['unique_route_material_memory_changes']} distinct changes in the "
        "route, material or memory settings. Full policies also contain instruction text. Different wording can "
        "therefore increase the full-policy count without changing a control decision.", "",
        "| Lane | Replayed / planned | Substantive / replayed | Adoptions | Holds | Extra substantive / extra replayed | Distinct eligible planned + extra |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for domain, lane in lanes.items():
        base, addition = lane["planned"], lane["contingency"]
        lines.append(f"| [{domain}](#{domain}) | {base['executed']}/{target} | "
                     f"{base['substantive']}/{base['executed']} | {base['adopted']} | {base['held']} | "
                     f"{addition['substantive']}/{addition['executed']} | "
                     f"{base['unique_operationally_complete']} + {addition['unique_operationally_complete']} |")
    u = Counter(report["usage"])
    attempts = u["accepted_receipts"] + u["failed_attempts"]
    lines += ["", "## Operational completion rule", "",
              "The reviewer must judge the change substantive. The forecast must have been supported or refuted "
              "by the overall review for protocols 1–2, or by the registered stage codes for protocol 3. At least "
              "one visit must differ in the actual route, released material or presence of authorized memory. "
              "The return displays must differ and each participant must have supplied a return reply. A declined "
              "task counts as a reply and gives no evidence of attainment.", "",
              "Within each domain, the fingerprint records the ordered incumbent and candidate paths. For each "
              "visit it binds the route, digest of the exact released material and whether any permitted memory "
              "reached the tutor. It also binds the registered stage when available. Learner wording, generated "
              "phrasing, ordinal and graded outcome contribute no identity. The first eligible use claims the "
              "fingerprint. Later eligible repeats stay visible but receive no additional distinct count. "
              "Semantic duplicates across different fingerprints require editorial review.", "",
              f"Of {all_rows['executed']} replayed pairs, {all_rows['executed'] - all_rows['operationally_complete']} "
              f"failed at least one criterion and {all_rows['operationally_complete'] - all_rows['unique_operationally_complete']} "
              "additional eligible pairs repeated an earlier fingerprint in their domain. Exclusion reasons overlap.", ""]
    for key, count in sorted(all_rows["completion_exclusions"].items()):
        lines.append(f"- {key.replace('_', ' ')} applied to {count} pairs.")
    lines += ["", "## Common comparison task", ""]
    for p, group in report["by_protocol"].items():
        lines.append(f"- Protocol {p} supplied both simulated participants with a common comparison in "
                     f"{group['shared_comparison_observed']}/{group['executed']} replayed pairs. "
                     f"Reviewers marked {group['observations'].get('supported', 0)} predictions supported, "
                     f"{group['observations'].get('refuted', 0)} refuted and "
                     f"{group['observations'].get('unobserved', 0)} unobserved.")
        if group["witnesses"]:
            results = Counter(group["witness_results"])
            lines += ["", f"For {group['witnesses']} protocol-{p} comparisons the designer knew the opening request "
                      "and registered a single feature to look for at a named stage. The reviewer coded that "
                      "feature in both arms and quoted the corresponding message in each. The host checked the "
                      f"registered direction against those codes. {results['supported']} contrasts were supported, "
                      f"{results['refuted']} refuted and {results['unobserved']} unresolved. In "
                      f"{group['witness_review_disagreements']} pairs the overall reviewer judgment differed "
                      "from that stage-specific result. Exact quotation checks establish location. The coder's "
                      "interpretation of the quotation remains a judgment.", "",
                      f"Within the lanes, {group['repeated_witness_definitions_in_lane']} registrations repeated "
                      "the exact stage, direction and difference text of an earlier one. This check cannot detect "
                      f"semantic duplicates. Reviewers judged {group['reviewer_non_substantive']}/{group['executed']} "
                      "pairs non-substantive. Those pairs remain in the execution denominator.", ""]
    if not report["by_protocol"]:
        lines.append("No comparison protocol has a replayed decision yet.")
    lines += ["", "Across protocols, paired responses to different return tasks offer evidence about what each "
              "policy elicited. Only a shared comparison fixes the task for both arms. Independent human scoring "
              "and a real learner population remain necessary to measure attainment.", ""]
    for setting, group in report["by_setting"].items():
        lines.append(f"- {setting.capitalize()} was the scenario in {group['executed']} replayed pairs. "
                     f"Reviewers marked {group['substantive']} substantive and {group['adopted']} were adopted.")
    lines += ["", "## Recorded resource use", "",
              f"{u['accepted_receipts']:,} accepted stage receipts and {u['failed_attempts']:,} failed attempts "
              "had readable telemetry, including staged work in unfinished rounds. ",
              f"Reported elapsed time summed to {u['reported_call_seconds']:,.1f} call-seconds. Concurrent "
              "calls overlap in wall time.", "",
              f"Among {u['failed_attempts']} failed attempts, {u['no_executable_policy_change']} designs "
              f"were ineligible because their executable choices stayed the same. In {u['quote_not_in_registered_stage']} "
              "review attempts the supplied quotation did not occur at the registered stage. A successful retry "
              "does not erase the earlier resource use.", "",
              "| Counter | Reported sum | Attempts with a counter |", "|---|---:|---:|"]
    for key in TOKENS:
        n = u[key + "_receipts"]
        lines.append(f"| {key} | {u[key]:,} | {n}/{attempts} |" if n else f"| {key} | unavailable | 0/{attempts} |")
    lines += ["", "Cached input can be a subset of input usage. These counters do not establish a subscriber bill or "
              "the cost of serving a classroom. Unreported calls and incomplete telemetry can increase the totals.", "",
              "| Role | Use | Accepted / failed | Accepted p50 / p95 seconds | Input / cached input / output tokens on accepted calls |",
              "|---|---|---:|---:|---:|"]
    for role, group in sorted(report["roles"].items(), key=lambda pair: (pair[1]["purpose"] != "tutor delivery", pair[0])):
        ok, failed = Counter(group["accepted"]), Counter(group["failed"])
        times = " / ".join("unavailable" if group[k] is None else f"{group[k]:.1f}"
                           for k in ("p50_accepted_seconds", "p95_accepted_seconds"))
        usage = " / ".join(f"{ok[k]:,} ({ok[k+'_receipts']}/{ok['calls']})" if ok[k+"_receipts"] else "unavailable"
                           for k in ("input_tokens", "cached_input_tokens", "output_tokens"))
        lines.append(f"| {role} | {group['purpose']} | {ok['calls']} / {failed['calls']} | {times} | {usage} |")
    routes = Counter(planned["instruction_routes"]) + Counter(extra["instruction_routes"])
    lines += ["", f"The {sum(routes.values())} first-visit and return displays across replayed arms used "
              f"{routes['card']} local card routes and {routes['coach']} model coach routes. "
              "The route counts measure delivery. Educational usefulness requires a separate judgment. Every "
              "arm also commissioned feedback. Role latency uses nearest-rank percentiles of individual accepted "
              "CLI calls, including process and transport overhead. Failed calls have separate usage totals in "
              "the machine report. These measurements do not give a classroom or end-to-end request p95.", "",
              "## What the evidence establishes", "",
              "The replay checks the course pin, preceding decision, precommitted proposal, both teaching paths, "
              "their memory and return packets, the reviewer's packet and the inherited disposition. The fictional learner "
              "received the display the host assembled. The reviewer and learner were simulated with the same model. "
              "Classroom settings describe a situation for one participant. Class-wide throughput, independent human "
              "learning gains and competitive superiority remain unmeasured.", "",
              "Each lane lists every replayed decision in order. For the first adoption and the first hold in each lane, "
              "the short quotations reproduce the beginning of the common-task replies where available, or each "
              "participant's own return reply. Full records and usage receipts "
              "are organized by lane and ordinal in the [evidence archive](evidence.zip)."]
    for domain, lane in lanes.items():
        lines += ["", f"## {domain}", ""]
        rows = lane["rounds"]
        if lane["missing_planned"]:
            lines += ["Missing replayed planned decisions at " + ", ".join(f"{n:03d}" for n in lane["missing_planned"]) + ".", ""]
        if lane["pending"]:
            lines += ["Incomplete checkpoint folders include " + ", ".join(p["id"] for p in lane["pending"]) + ".", ""]
        if not rows:
            continue
        lines += ["| Ordinal | Previous | Tested change | Reviewer prediction | Registered contrast | Substantive | Decision | Operational count |",
                  "|---:|---|---|---|---|---|---|---|"]
        for r in rows:
            mechanism = html.escape(r["mechanism"], quote=False).replace("|", "&#124;").replace("\n", " ")
            n = r["ordinal"]
            previous = f"[{n - 1:03d}](#{domain}-round-{n - 1:03d})" if n > 1 else "genesis"
            coded = r["witness"]["result"] if r["witness"] else "—"
            if r["completion_exclusions"]:
                credit = "excluded"
            elif earlier := r.get("previous_eligible_fingerprint"):
                prior = earlier.rsplit("/", 1)[1]
                credit = f"repeat of [{prior}](#{domain}-round-{prior})"
            else:
                credit = "included"
            lines.append(f'| <a id="{domain}-round-{n:03d}"></a>{n:03d} | {previous} | {mechanism} | {r["observed"]} | {coded} | '
                         f"{'yes' if r['substantive'] else 'no'} | {r['disposition']} | {credit} |")
        picks = {}
        for r in rows:
            picks.setdefault(r["disposition"], r)
        focused = next((r for r in rows if r["witness"] and r not in picks.values()), None)
        if focused:
            picks["first stage-coded comparison"] = focused
        for r in picks.values():
            field = "comparison_reply" if r["common_comparison"] else "return_reply"
            when = "on the shared comparison" if r["common_comparison"] else "on return"
            lines += ["", f"### {domain} {r['ordinal']:03d}", "", html.escape(r["question"]), "",
                      f"Incumbent learner {when}", "", quoted(r["verbatim"]["incumbent"][field]),
                      "", f"Candidate learner {when}", "", quoted(r["verbatim"]["candidate"][field]),
                      "", "Reviewer finding", "", quoted(r["finding"], 420)]
            if r["witness"]:
                w = r["witness"]
                lines += ["", f"The preregistered feature at `{w['stage']}` was expected in the "
                          f"{w['direction'].replace('_only', '')} arm alone.", "",
                          quoted(w["difference"], 320), "",
                          f"Candidate coded **{w['candidate']['present']}**", "", quoted(w["candidate"]["quote"]),
                          "", f"Incumbent coded **{w['incumbent']['present']}**", "", quoted(w["incumbent"]["quote"]),
                          "", f"Registered contrast **{w['result']}**."]
    if report["audit_issues"]:
        lines += ["", "## Records requiring inspection", "", *("- " + entry for entry in report["audit_issues"])]
    return "\n".join(lines).rstrip() + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--target-per-domain", type=int, default=30)
    args = parser.parse_args()
    if not 1 <= args.target_per_domain <= 1000:
        parser.error("target must be between 1 and 1000")
    if args.output.resolve().is_relative_to(args.root.resolve()):
        parser.error("choose an output directory outside the run")
    lanes, usage, issues, files, roles = scan(args.root, args.target_per_domain)
    report = summary(lanes, usage, issues, args.target_per_domain, roles)
    args.output.mkdir(parents=True, exist_ok=True)
    docs = {"summary.json": encoded(report), "summary.md": markdown(report).encode()}
    for name, data in docs.items():
        package_member(files, name, data)
    atomic(args.output / "evidence.zip", archive(files))
    for name, data in docs.items():
        atomic(args.output / name, data)
    print(f"{report['planned']['executed']} planned and {report['contingency']['executed']} extra decisions replayed; "
          f"{len(issues)} audit issues. {args.output}")
    if issues:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
