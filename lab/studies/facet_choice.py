"""Compare two actual tutors on a learner-elected facet under a fixed practice-only request."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from hashlib import sha256
from pathlib import Path

from teaching_trial import pack, read, save
from water_teaching import call

TUTOR = ("Teach only from the offered activity and what the learner says. Name the available exercise "
         "and give a concise usable teaching response. If the offered exercise cannot serve today's request, "
         "explain what is missing. Do not invent a release, a performance observation, or a booking.")
REQUEST = ("I want to stay with violin. Today I want to try making a phrase arrive somewhere. "
           "Can I try that on open strings, without recording anything? Please keep my pulse review for another day.")


def compare(campaign, release_path, destination):
    course = pack("violin")
    release = read(release_path)
    unit = next(unit for unit in course["proposal"]["units"] if unit["id"] == release["unit"])
    if (unit["facet"] != release["facet"] or unit["title"] != release["title"]
            or release["rights"] != "original lab exercise" or release["use"] != "formative practice"):
        raise ValueError("elective release does not fit the declared unit")
    record = campaign / "033" / "A-resume.json"
    prior = read(record)
    if prior["packet"]["goal"] != course["scope"]["goal"] or "foot tap" not in prior["response"]["display"]:
        raise ValueError("study requires the archived pulse-feedback encounter")
    common = {"current_goal": course["scope"]["goal"], "request": REQUEST,
              "prior_review": "Pulse and re-entry review is parked for today.",
              "evidence_limit": "Voluntary off-record practice. No observation of playing is available."}
    existing = course["return_pack"]
    slots = {"pulse": {"unit": existing["unit"], "exercise": existing["practice"]},
             "elective": {"unit": release["unit"], "exercise": release["exercise"]}}
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("x", encoding="utf-8") as out:
        out.write("{}\n")
    result = {"ancestor_sha256": sha256(record.read_bytes()).hexdigest(),
              "elective_sha256": sha256(release_path.read_bytes().replace(b"\r\n", b"\n")).hexdigest(),
              "tutor_instruction": TUTOR, "arms": {}}
    with ThreadPoolExecutor(max_workers=2) as pool:
        jobs = {name: pool.submit(call, {**common, "available_activity": item}, TUTOR, ["display"])
                for name, item in slots.items()}
        for name, job in jobs.items():
            result["arms"][name] = job.result()
            save(destination, result)


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("campaign", type=Path)
    p.add_argument("release", type=Path)
    p.add_argument("output", type=Path)
    p.add_argument("--live", required=True, action="store_true")
    a = p.parse_args()
    compare(a.campaign, a.release, a.output)
