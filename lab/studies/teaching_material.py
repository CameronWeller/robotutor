"""Read the pinned teaching packs shared by the rehearsals and the learner pilot."""
import hashlib
import json
from pathlib import Path


PACKS = Path(__file__).resolve().parents[1] / "fixtures" / "goals"
DOMAINS = ("writer", "chemical-engineering", "violin", "better-person", "worship")


def pack(domain):
    if domain not in DOMAINS:
        raise ValueError("unknown teaching pack")
    data = (PACKS / (domain + ".json")).read_bytes().replace(b"\r\n", b"\n")
    pins = json.loads((PACKS / "reviews.json").read_text(encoding="utf-8"))
    if hashlib.sha256(data).hexdigest() != pins[domain]:
        raise ValueError("unreviewed teaching pack")
    return json.loads(data)
