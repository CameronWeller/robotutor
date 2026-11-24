"""Opt-in public source discovery on USGS, emitting candidate metadata only."""
import argparse
from collections import Counter, deque
from hashlib import sha256
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urldefrag, urljoin, urlsplit
from urllib.request import build_opener, HTTPRedirectHandler, Request
from urllib.robotparser import RobotFileParser


ORIGIN = "https://www.usgs.gov"
PROGRAM = ORIGIN + "/centers/pennsylvania-water-science-center/science/groundwater-availability-and-use-pennsylvania"
AGENT = "RobotutorSourceStudy/1.0"
PAGES, LINKS, BODY, TOTAL, ROBOTS = 4, 256, 512 * 1024, 1024 * 1024, 8192
FIXTURES = Path(__file__).resolve().parents[1] / "fixtures" / "goals"


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class MainText(HTMLParser):
    """Only titled main content and paragraph text enter the normalized text edition."""

    def __init__(self, edge_limit):
        super().__init__(convert_charrefs=True)
        self.edge_limit = edge_limit
        self.main = False
        self.paragraph = None
        self.paragraphs = []
        self.title_parts = []
        self.in_title = False
        self.edges = []
        self.truncated = False

    def handle_starttag(self, tag, attrs):
        if tag == "main":
            self.main = True
        if not self.main:
            return
        if tag == "h1":
            self.in_title = True
        elif tag == "p":
            self.paragraph = []
        elif tag == "a" and (href := dict(attrs).get("href")):
            if len(self.edges) < self.edge_limit:
                self.edges.append(href)
            else:
                self.truncated = True

    def handle_endtag(self, tag):
        if tag == "p" and self.paragraph is not None:
            line = " ".join("".join(self.paragraph).split())
            if line:
                self.paragraphs.append(line)
            self.paragraph = None
        elif tag == "h1":
            self.in_title = False
        elif tag == "main":
            self.main = False

    def handle_data(self, data):
        if self.in_title:
            self.title_parts.append(data)
        if self.paragraph is not None:
            self.paragraph.append(data)

    def document(self):
        title = " ".join("".join(self.title_parts).split())
        return title, "\n".join([title, *self.paragraphs]).encode("utf-8")


def supported_url(url):
    parts = urlsplit(url)
    return (parts.scheme == "https" and parts.netloc == "www.usgs.gov" and not parts.query
            and (re.fullmatch(r"/publications/[a-z0-9-]+", parts.path) is not None or url == PROGRAM))


def relevance(url):
    slug = urlsplit(url).path
    if "water-budget" in slug or "water-balance" in slug:
        return 0
    if "recharge" in slug:
        return 1
    return 2


def first_pack():
    raw = (FIXTURES / "chemical-engineering.json").read_bytes().replace(b"\r\n", b"\n")
    pins = json.loads((FIXTURES / "reviews.json").read_bytes())
    if sha256(raw).hexdigest() != pins["chemical-engineering"]:
        raise ValueError("chemical engineering fixture differs from its review")
    return json.loads(raw)["first_pack"]


def find_span(document, quote):
    needle = quote.encode("utf-8")
    start = document.find(needle)
    if start < 0 or document.find(needle, start + 1) >= 0:
        return None
    return {"document": sha256(document).hexdigest(), "start": start, "end": start + len(needle)}


def revalidate(previous, document, quote):
    if document is None:
        return "source unavailable"
    found = find_span(document, quote)
    if found is None:
        return "passage absent or ambiguous"
    if found != previous:
        return "new edition requires review"
    return "same passage and edition"


def walk():
    pack = first_pack()
    if not supported_url(pack["source_url"]):
        raise ValueError("reviewed opening source is outside this collection")
    opener = build_opener(NoRedirect())
    read_bytes = 0

    def fetch(url, content_type, limit):
        nonlocal read_bytes
        budget = min(limit, TOTAL - read_bytes)
        if budget <= 0:
            raise ValueError("body allowance exhausted")
        request = Request(url, headers={"User-Agent": AGENT, "Accept": content_type,
                                        "Accept-Encoding": "identity"})
        with opener.open(request, timeout=12) as response:
            if (response.geturl() != url or response.headers.get_content_type() != content_type
                    or response.headers.get("Content-Encoding", "identity").lower() != "identity"):
                raise ValueError("response outside permitted representation")
            # Count each read immediately, including a partial body if a later read fails.
            data = bytearray()
            while len(data) < budget:
                chunk = response.read(min(16384, budget - len(data)))
                if not chunk:
                    return bytes(data), response.headers.get_content_charset() or "utf-8"
                data.extend(chunk)
                read_bytes += len(chunk)
            raise ValueError("response reached byte cap")

    robots_url = ORIGIN + "/robots.txt"
    policy, encoding = fetch(robots_url, "text/plain", ROBOTS)
    robots = RobotFileParser(robots_url)
    robots.parse(policy.decode(encoding).splitlines())
    delay = max(1, robots.crawl_delay(AGENT) or 0)
    rate = robots.request_rate(AGENT)
    if rate:
        delay = max(delay, rate.seconds / rate.requests)
    frontier = deque([(pack["source_url"], None), (PROGRAM, None)])
    known = {url for url, _ in frontier}
    counters = Counter()
    pages = []
    remaining = LINKS
    comparison = {}
    started = time.monotonic()
    while frontier and len(pages) < PAGES and read_bytes < TOTAL:
        url, parent = frontier.popleft()
        record = {"url": url, "discovered_from": parent}
        pages.append(record)
        if not robots.can_fetch(AGENT, url):
            record["status"] = "robots rules withheld request"
            continue
        time.sleep(delay)
        try:
            body, encoding = fetch(url, "text/html", BODY)
            parsed = MainText(remaining)
            parsed.feed(body.decode(encoding))
            parsed.close()
            title, document = parsed.document()
            if not title or not parsed.paragraphs:
                raise ValueError("missing main title or paragraphs")
        except HTTPError as exc:
            record["status"] = f"HTTP {exc.code}"
            continue
        except (URLError, OSError, UnicodeError, ValueError) as exc:
            record["status"] = f"unusable response: {type(exc).__name__}"
            continue
        record.update(status="candidate", title=title[:180], body_bytes=len(body),
                      document=sha256(document).hexdigest(), normalized_bytes=len(document),
                      inspected_links=len(parsed.edges), more_links=parsed.truncated)
        remaining -= len(parsed.edges)
        if url == pack["source_url"]:
            quote = pack["excerpt"]
            span = find_span(document, quote)
            if span:
                record.update(span=span, excerpt=quote)
                comparison = {
                    "observed": revalidate(span, document, quote),
                    "controlled_addition": revalidate(span, b"Revised heading\n" + document, quote),
                    "controlled_removal": revalidate(span, document.replace(quote.encode(), b""), quote),
                    "unavailable": revalidate(span, None, quote),
                }
        leads = []
        for edge in parsed.edges:
            target = urldefrag(urljoin(url, edge))[0]
            if not supported_url(target):
                counters["outside collection"] += 1
            elif target in known:
                counters["already known"] += 1
            elif relevance(target) == 2:
                counters["outside topic"] += 1
            else:
                known.add(target)
                leads.append(target)
                counters["admitted leads"] += 1
        frontier.extend((link, url) for link in sorted(leads, key=relevance))
    return {"source_policy": robots_url, "robots_bytes": len(policy), "minimum_interval_s": delay,
            "limits": {"pages": PAGES, "link_checks": LINKS, "page_bytes": BODY, "total_bytes": TOTAL},
            "read_bytes": read_bytes, "walk_seconds": round(time.monotonic() - started, 2),
            "link_outcomes": dict(counters), "pending_leads": [url for url, _ in frontier],
            "pages": pages, "anchor_controls": comparison}


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--live", action="store_true", required=True, help="fetch a few public USGS pages")
    cli.parse_args()
    print(json.dumps(walk(), indent=2, ensure_ascii=True))
