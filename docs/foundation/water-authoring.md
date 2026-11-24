# Can a generated water-inventory question be scored?

The author produced three correct numerical keys and withheld a fourth when the rate readings had no duration. A fluent prompt still existed for the underdetermined item. The study therefore treats answerability as a release decision before presenting a scored question. [Runnable scorer and replay](../../lab/examples/water_author.rs)

## What the author saw

Two independent authenticated Codex CLI processes requested `gpt-6.2-sol` concurrently. Each received one JSON packet with the existing chemical-engineering criterion, two reviewed water-budget excerpts, the ledger IDs, the water boundary, the elapsed minutes or an explicit null, and every flow's endpoints, amount and unit. The instruction asked for one fluent question per ledger, the proposed signed key in kg or null, and the crossing flows in ledger order. The schema also required the item ID, boundary, duration and answer unit. The model received no learner history, course answer key or browser results. The reported input-token totals include the CLI runtime context and schema as well as the short study packet.

| Group | Cases | Wall time | Reported input / output tokens |
| --- | --- | ---: | ---: |
| First | Single tank with a known duration and the same readings with duration missing | 20.361 s | 18,914 / 259 |
| Return | Changed readings with the same answer and two basins with an internal transfer | 20.701 s | 18,935 / 281 |

Elapsed time for both calls together was 23.798 seconds. The run used two author calls, zero JEV calls and zero reviewer model calls. It yielded no dollar charge measurement. The receipt hashes cover the exact UTF-8 JSON prompts without a trailing newline. The attached responses are model-authored research data.

## Release and learner consequences

| Ledger | Model answer | What the checked release does |
| --- | --- | --- |
| T1 · one tank, 4 minutes | +8 kg | Two external rates give `(4 − 2) × 4 = +8`. The learner can practice on the current pack. A correct answer after help carries assistance into the evidence record. |
| T2 · one tank, duration missing | null | The model still wrote a question asking for the interval mass. The release withholds it because `2 kg/min` needs a duration to yield kg. The honest author also supplied no scoreable key. |
| T3 · one tank, 2 minutes | +8 kg | `(7 − 3) × 2 = +8`. Someone can repeat T1's response unchanged. The checked exercise remains available for practice. This pair earns no evidence of transfer. |
| P1 · one boundary around two basins | −4 kg | The six-minute inlet contributes +6 kg. The outlet removes 10 kg. The west-to-east transfer of 9 kg stays inside. The correct signed change is −4 kg. A repeated +8 kg is marked incorrect. |

Both grading paths accept the authentic P1 answer. As a controlled fault, the replay adds the internal 9 kg to the key while preserving the exact fluent question. Trusting the proposed key then scores +5 kg correct and −4 kg incorrect. Independent admission rejects that candidate. This fault was injected into a copy of the receipt; it was never an author response.

The checked route compares the proposed boundary, duration, unit, crossing IDs and key with a separately supplied ledger. It renders the actual question from that ledger. The rubric and answer also come from the signed mass balance. This avoids making a prose-entailment claim about the model's draft. The lab's existing `Batch` holds the pack until a separate simulated review actor admits the exact digest. Removing the ledger pin blocks display. Withdrawing the approved row blocks a later correct submission.

In the day-30 replay, a correct P1 response following an unassisted T1 observation satisfied the existing spacing rule only for the explicitly reviewed pair, given complete exposure history. A guided T1 success was ineligible. Adding a day-29 encounter changed the day-30 result to insufficient spacing. These are controlled scheduling outcomes. The experiment used synthetic responses and supplies no observation of learner retention, practical engineering competence, or the correctness of arbitrary generated prose.

Three Rust tests passed. The arithmetic check enumerated 125 combinations of inlet rate, internal transfer mass and outlet mass over six minutes. Each combined inventory change equaled the sum of the individual basin changes. A Telo monitor checked 192 read boundaries in 64 three-step grant and withdrawal histories. Its property required a scored submission exactly when the source grant was current and the row had survived withdrawal. A fabricated score after grant loss failed. The checks assume complete movement records, a fixed boundary and constant rates; none of those assumptions can be recovered from an arbitrary learner's free-text problem.

```sh
cargo run --locked --manifest-path lab/Cargo.toml --example water_author -- --packet first
cargo run --locked --manifest-path lab/Cargo.toml --example water_author -- --packet return
cargo run --locked --manifest-path lab/Cargo.toml --example water_author -- --replay docs/foundation/water-authoring.md
cargo test --locked --manifest-path lab/Cargo.toml --example water_author
```

## Live receipts

```json
{
  "wall_seconds": 23.798,
  "calls": [
    {
      "group": "first",
      "requested_model": "gpt-6.2-sol",
      "request_sha256": "92d8520d8c83122bc2d2c22236c88b736bc375a8d7a72bee5ee5380e1e934f35",
      "seconds": 20.361,
      "usage": {
        "input_tokens": 18914,
        "cached_input_tokens": 0,
        "cache_write_input_tokens": 0,
        "output_tokens": 259,
        "reasoning_output_tokens": 0
      },
      "items": [
        {
          "id": "T1",
          "draft_question": "Over a 4-minute interval, the feed carries water from outside into west at 4 kg/min, and the drain carries water from west to outside at 2 kg/min. What is the signed change in total water inventory, in kg, within the west boundary over that interval?",
          "boundary": [
            "west"
          ],
          "interval_min": 4,
          "answer_unit": "kg",
          "crossing_flows": [
            "feed",
            "drain"
          ],
          "key_kg": 8
        },
        {
          "id": "T2",
          "draft_question": "Over an interval of unspecified duration, the feed carries water from outside into west at 4 kg/min, and the drain carries water from west to outside at 2 kg/min. What is the signed change in total water inventory, in kg, within the west boundary over that interval?",
          "boundary": [
            "west"
          ],
          "interval_min": null,
          "answer_unit": "kg",
          "crossing_flows": [
            "feed",
            "drain"
          ],
          "key_kg": null
        }
      ]
    },
    {
      "group": "return",
      "requested_model": "gpt-6.2-sol",
      "request_sha256": "491c34acf56a94619ef0712ae81af5d0ddfe30f35578313db42d07946b55ff52",
      "seconds": 20.701,
      "usage": {
        "input_tokens": 18935,
        "cached_input_tokens": 0,
        "cache_write_input_tokens": 0,
        "output_tokens": 281,
        "reasoning_output_tokens": 0
      },
      "items": [
        {
          "id": "T3",
          "draft_question": "Over a 2-minute interval, the feed carries water from outside into west at 7 kg/min, and the drain carries water from west to outside at 3 kg/min. For the boundary enclosing west, what is the signed change in total water inventory over the interval, in kilograms?",
          "boundary": [
            "west"
          ],
          "interval_min": 2,
          "answer_unit": "kg",
          "crossing_flows": [
            "feed",
            "drain"
          ],
          "key_kg": 8
        },
        {
          "id": "P1",
          "draft_question": "Over a 6-minute interval, the feed carries water from outside into west at 1 kg/min, the transfer moves 9 kg of water from west to east, and the drain carries 10 kg of water from east to outside. For the boundary enclosing both west and east, what is the signed change in total water inventory over the interval, in kilograms?",
          "boundary": [
            "west",
            "east"
          ],
          "interval_min": 6,
          "answer_unit": "kg",
          "crossing_flows": [
            "feed",
            "drain"
          ],
          "key_kg": -4
        }
      ]
    }
  ]
}
```
