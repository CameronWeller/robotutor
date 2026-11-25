# Estimating a learner’s unaided recall

Robotutor reconstructs one forecast per objective in the private learner workspace. A reviewed claim or a card and stage gives an objective a stable identity across visits. For other lessons the topic and objective identify it. Passage search supplies prior conversation. The recall forecast uses the structured history of attempts, dates, assistance and validation.

For an elapsed gap of $d$ days, the prediction is

$$P(\text{unaided success at }d)=a\sum_i w_i\left(1+\frac{d}{9S_i}\right)^{-1}.$$

The initial stability grid $S_i$ is 2, 7, 14, 28 and 60 days, with weights 0.1, 0.2, 0.4, 0.2 and 0.1. Each single-stability curve uses the existing retention model. The availability factor $a$ starts at 0.95 after a verified success and 0.35 after a verified miss. These constants are initial engineering assumptions. Human learner calibration remains open.

Before incorporating a fresh delayed check, the tutor scores its existing probability against the outcome. For success it multiplies each weight by that curve’s predicted probability. For a miss it uses one minus that probability. It normalizes the weights and anchors the next forecast to the new check. Same-day checks can supply a new anchor. Only positive gaps without intervening recorded teaching or ambiguous work update the stability weights. Reused prompts, assisted work, undated responses and unverified judgments suspend the estimate until a new dated independent check. The exposure history covers this workspace. Activities outside it can affect actual performance.

The learner can inspect the estimate and dated source with `/skills N`. On eligible delayed checks this view reports mean squared probability error (Brier score) for both the personalized forecast and a fixed 14-day curve using the same availability factor. Lower is better. The model also supplies the range between the tenth and ninetieth percentiles over the stability grid to Luna. This describes sensitivity to the grid and initial assumptions.

An eligible forecast suggests the first whole day its predicted probability falls to 0.85. Existing changed-form checkpoints still govern their own return dates. Durable evidence retains its independent qualification rules. Luna uses recent independent success to vary the task, uses a long gap or miss to invite retrieval, and grounds credit in the learner’s actual answer. The ordinary teaching packet gives the active objective priority among the eight available skill records. The speculative practice path can use the same forecast before the learner answers. Both paths reuse their existing Luna requests.

Luna sees the learner’s goal alongside the preceding task and criterion. A local review check catches an explicit multiplication or division of the new exercise’s two quantities in the teaching text. It sends the draft through the existing revision path before release. For a verbal or conceptual cue, the existing Luna review returns the exact learner-visible phrase that supplied an assessed step. A matching quote marks the upcoming exercise as assisted. Its answer cannot train the unaided forecast. Reviewer omissions remain possible.
