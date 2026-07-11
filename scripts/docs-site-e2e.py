#!/usr/bin/env python3
"""Build and validate the rendered Docusaurus documentation site."""

from __future__ import annotations

import argparse
from html.parser import HTMLParser
import html
from pathlib import Path
import re
import struct
import subprocess
import sys
import xml.etree.ElementTree as ET
from urllib.parse import unquote, urljoin, urlparse


ROOT = Path(__file__).resolve().parents[1]
WEBSITE = ROOT / "website"
BUILD = WEBSITE / "build"
DOCS = ROOT / "docs"
SIDEBAR_JS = WEBSITE / "sidebars.js"
HOMEPAGE_TSX = WEBSITE / "src" / "pages" / "index.tsx"
DOCUSAURUS_CONFIG = WEBSITE / "docusaurus.config.js"
ARCHITECTURE_DOC = DOCS / "ARCHITECTURE.md"
I18N = WEBSITE / "i18n"

SITE_ORIGIN = "https://taeyun16.github.io"
BASE_PATH = "/aidememo/"
LOCALES = {
    "en": {"base_path": BASE_PATH, "html_lang": "en-US"},
    "ko": {"base_path": f"{BASE_PATH}ko/", "html_lang": "ko-KR"},
}
HOMEPAGE_H1 = {
    "en": "AideMemo",
    "ko": "AideMemo",
}

SOURCE_PATH_PREFIXES = (
    "aidememo-skill/",
    "bench/",
    "benchmarks/",
    "crates/",
    "docs/",
    "packages/",
    "plugins/",
    "scripts/",
    "website/",
    ".github/",
)


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.title_parts: list[str] = []
        self.h1_parts: list[str] = []
        self.ids: set[str] = set()
        self.hrefs: list[str] = []
        self.srcs: list[str] = []
        self.html_lang = ""
        self.alternates: set[tuple[str, str]] = set()
        self.metadata: dict[str, str] = {}
        self.links: set[tuple[str, str]] = set()
        self._in_title = False
        self._h1_depth = 0

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attr_map = {name: value or "" for name, value in attrs}
        if tag == "html":
            self.html_lang = attr_map.get("lang", "")
        elif tag == "title":
            self._in_title = True
        elif tag == "h1":
            self._h1_depth += 1
        elif tag == "meta":
            key = attr_map.get("property") or attr_map.get("name")
            if key and attr_map.get("content"):
                self.metadata[key] = attr_map["content"]
        if "id" in attr_map:
            self.ids.add(attr_map["id"])
        if "name" in attr_map:
            self.ids.add(attr_map["name"])
        if "href" in attr_map:
            self.hrefs.append(attr_map["href"])
            if tag == "link" and attr_map.get("rel"):
                self.links.add((attr_map["rel"], attr_map["href"]))
            if tag == "link" and attr_map.get("rel") == "alternate" and attr_map.get("hreflang"):
                self.alternates.add((attr_map["hreflang"], attr_map["href"]))
        if "src" in attr_map:
            self.srcs.append(attr_map["src"])

    def handle_endtag(self, tag: str) -> None:
        if tag == "title":
            self._in_title = False
        elif tag == "h1" and self._h1_depth:
            self._h1_depth -= 1

    def handle_data(self, data: str) -> None:
        if self._in_title:
            self.title_parts.append(data)
        if self._h1_depth:
            self.h1_parts.append(data)

    @property
    def title(self) -> str:
        return normalize_text("".join(self.title_parts))

    @property
    def h1(self) -> str:
        return normalize_text("".join(self.h1_parts))


def normalize_text(value: str) -> str:
    return re.sub(r"\s+", " ", html.unescape(value)).strip()


def fail(errors: list[str], message: str) -> None:
    errors.append(message)


def run_build() -> None:
    subprocess.check_call(["npm", "--prefix", str(WEBSITE), "run", "build"], cwd=ROOT)


def sidebar_doc_ids() -> list[str]:
    text = SIDEBAR_JS.read_text(encoding="utf-8")
    return sorted(dict.fromkeys(re.findall(r"['\"]([A-Z][A-Z0-9_]+)['\"]", text)))


def homepage_doc_routes() -> list[str]:
    text = HOMEPAGE_TSX.read_text(encoding="utf-8")
    return sorted(dict.fromkeys(re.findall(r"to:\s*['\"](/docs/[A-Z0-9_]+)['\"]", text)))


def sitemap_routes(locale: str) -> list[str]:
    sitemap = BUILD / ("sitemap.xml" if locale == "en" else f"{locale}/sitemap.xml")
    root = ET.fromstring(sitemap.read_text(encoding="utf-8"))
    routes: list[str] = []
    ns = {"sm": "http://www.sitemaps.org/schemas/sitemap/0.9"}
    for loc in root.findall(".//sm:loc", ns):
        parsed = urlparse(loc.text or "")
        routes.append(parsed.path)
    return sorted(dict.fromkeys(routes))


def route_to_file(route: str) -> Path:
    if not route.startswith(BASE_PATH):
        raise ValueError(f"route {route!r} does not start with {BASE_PATH!r}")
    rel = route[len(BASE_PATH) :]
    if rel == "":
        return BUILD / "index.html"
    if rel.endswith("/"):
        return BUILD / rel / "index.html"
    candidate = BUILD / rel
    if candidate.suffix:
        return candidate
    return BUILD / f"{rel}.html"


def page_route_for_doc(doc_id: str, locale: str) -> str:
    return f"{LOCALES[locale]['base_path']}docs/{doc_id}"


def locale_for_route(route: str) -> str:
    for locale, config in reversed(list(LOCALES.items())):
        if route.startswith(config["base_path"]):
            return locale
    raise ValueError(f"route {route!r} does not match a configured locale")


def doc_source_for_route(route: str, doc_id: str) -> Path:
    locale = locale_for_route(route)
    if locale != "en":
        translated = I18N / locale / "docusaurus-plugin-content-docs" / "current" / f"{doc_id}.md"
        if translated.exists():
            return translated
    return DOCS / f"{doc_id}.md"


def route_content_path(route: str, locale: str) -> str:
    return route.removeprefix(str(LOCALES[locale]["base_path"]))


def first_markdown_h1(path: Path) -> str:
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("# "):
            return line[2:].strip()
    return ""


def parse_page(path: Path) -> PageParser:
    parser = PageParser()
    parser.feed(path.read_text(encoding="utf-8", errors="ignore"))
    return parser


def localize_url(current_route: str, raw: str) -> tuple[str, str] | None:
    if raw.startswith(("mailto:", "tel:", "javascript:", "data:")):
        return None
    if raw.startswith(("/docs/", "/img/", "/assets/")):
        return ("missing-base-url", raw)

    absolute = urljoin(f"{SITE_ORIGIN}{current_route}", raw)
    parsed = urlparse(absolute)
    if parsed.scheme not in ("http", "https") or parsed.netloc != urlparse(SITE_ORIGIN).netloc:
        return None
    if not parsed.path.startswith(BASE_PATH):
        return ("missing-base-url", raw)
    return (parsed.path, parsed.fragment)


def target_path_for_url_path(path: str) -> Path:
    if path == BASE_PATH:
        return BUILD / "index.html"
    rel = path[len(BASE_PATH) :]
    candidate = BUILD / rel
    if candidate.exists():
        return candidate
    if not candidate.suffix:
        return BUILD / f"{rel}.html"
    return candidate


def check_routes(errors: list[str]) -> tuple[dict[str, PageParser], list[str]]:
    doc_ids = sidebar_doc_ids()
    expected_routes: list[str] = []
    for locale, config in LOCALES.items():
        locale_expected = sorted(
            [str(config["base_path"]), *(page_route_for_doc(doc_id, locale) for doc_id in doc_ids)]
        )
        expected_routes.extend(locale_expected)
        routes = sitemap_routes(locale)
        missing_routes = sorted(set(locale_expected) - set(routes))
        extra_sidebar_missing = sorted(set(routes) - set(locale_expected))
        for route in missing_routes:
            fail(errors, f"{locale} sitemap is missing public route {route}")
        for route in extra_sidebar_missing:
            fail(
                errors,
                f"{locale} sitemap contains {route}, but it is not represented by homepage/sidebar contract",
            )

        homepage_routes = [
            f"{config['base_path']}{route.removeprefix('/')}" for route in homepage_doc_routes()
        ]
        for route in homepage_routes:
            if route not in locale_expected:
                fail(errors, f"{locale} homepage links to {route}, but sidebars.js does not expose that doc")

    expected_routes = sorted(expected_routes)

    pages: dict[str, PageParser] = {}
    for route in expected_routes:
        path = route_to_file(route)
        if not path.exists():
            fail(errors, f"route {route} did not build expected file {path.relative_to(ROOT)}")
            continue
        parser = parse_page(path)
        pages[route] = parser
        if "Page Not Found" in parser.title or "Page Not Found" in parser.h1:
            fail(errors, f"route {route} rendered a Docusaurus 404 page")
        if not parser.h1:
            fail(errors, f"route {route} rendered without an h1")
        locale = locale_for_route(route)
        expected_lang = str(LOCALES[locale]["html_lang"])
        if parser.html_lang != expected_lang:
            fail(errors, f"route {route} html lang {parser.html_lang!r} does not match {expected_lang!r}")

        content_path = route_content_path(route, locale)
        for alternate_locale, alternate_config in LOCALES.items():
            alternate_lang = str(alternate_config["html_lang"])
            alternate_path = f"{alternate_config['base_path']}{content_path}"
            alternate_url = f"{SITE_ORIGIN}{alternate_path}"
            if (alternate_lang, alternate_url) not in parser.alternates:
                fail(errors, f"route {route} is missing alternate {alternate_lang} -> {alternate_url}")
            if alternate_path not in parser.hrefs:
                fail(errors, f"route {route} locale dropdown is missing {alternate_locale} link {alternate_path}")
        default_url = f"{SITE_ORIGIN}{LOCALES['en']['base_path']}{content_path}"
        if ("x-default", default_url) not in parser.alternates:
            fail(errors, f"route {route} is missing x-default alternate -> {default_url}")

        if route == LOCALES[locale]["base_path"]:
            expected_h1 = HOMEPAGE_H1[locale]
            if parser.h1 != expected_h1:
                fail(errors, f"route {route} h1 {parser.h1!r} does not match {expected_h1!r}")
        else:
            doc_id = route.rsplit("/", 1)[-1]
            source = doc_source_for_route(route, doc_id)
            expected_h1 = first_markdown_h1(source)
            if expected_h1 and parser.h1 != expected_h1:
                fail(
                    errors,
                    f"route {route} h1 {parser.h1!r} does not match {source.relative_to(ROOT)} h1 {expected_h1!r}",
                )
    return pages, expected_routes


def check_links_and_assets(errors: list[str], pages: dict[str, PageParser]) -> tuple[int, int, int]:
    checked_links: set[tuple[str, str]] = set()
    checked_assets: set[str] = set()
    checked_anchors = 0

    for route, parser in list(pages.items()):
        for raw in parser.hrefs:
            localized = localize_url(route, raw)
            if localized is None:
                continue
            path_or_code, fragment = localized
            if path_or_code == "missing-base-url":
                fail(errors, f"{route}: href {raw!r} is root-relative without {BASE_PATH} baseUrl")
                continue
            target = target_path_for_url_path(path_or_code)
            if target.suffix in {".html", ""}:
                checked_links.add((route, path_or_code))
            else:
                checked_assets.add(path_or_code)
            if not target.exists():
                fail(errors, f"{route}: href {raw!r} points to missing build artifact {target.relative_to(ROOT)}")
                continue
            if fragment and target.suffix == ".html":
                target_route = path_or_code
                target_parser = pages.get(target_route)
                if target_parser is None:
                    target_parser = parse_page(target)
                    pages[target_route] = target_parser
                checked_anchors += 1
                decoded = unquote(fragment)
                if decoded not in target_parser.ids:
                    fail(errors, f"{route}: href {raw!r} points to missing anchor #{decoded} in {path_or_code}")

        for raw in parser.srcs:
            localized = localize_url(route, raw)
            if localized is None:
                continue
            path_or_code, _fragment = localized
            if path_or_code == "missing-base-url":
                fail(errors, f"{route}: src {raw!r} is root-relative without {BASE_PATH} baseUrl")
                continue
            target = target_path_for_url_path(path_or_code)
            checked_assets.add(path_or_code)
            if not target.exists():
                fail(errors, f"{route}: src {raw!r} points to missing build artifact {target.relative_to(ROOT)}")

    return len(checked_links), len(checked_assets), checked_anchors


def check_architecture_source_paths(errors: list[str]) -> int:
    text = ARCHITECTURE_DOC.read_text(encoding="utf-8")
    checked = 0
    for match in re.finditer(r"`([^`\n]+)`", text):
        token = match.group(1).strip().strip(".,:;")
        if " " in token or not token.startswith(SOURCE_PATH_PREFIXES):
            continue
        checked += 1
        if "*" in token:
            matches = list(ROOT.glob(token))
            if not matches:
                fail(errors, f"docs/ARCHITECTURE.md references glob {token!r}, but it matches no files")
            continue
        target = ROOT / token
        if not target.exists():
            fail(errors, f"docs/ARCHITECTURE.md references missing repo path {token!r}")
    return checked


def check_config(errors: list[str]) -> None:
    config = DOCUSAURUS_CONFIG.read_text(encoding="utf-8")
    if f"baseUrl: '{BASE_PATH}'" not in config and f'baseUrl: "{BASE_PATH}"' not in config:
        fail(errors, f"website/docusaurus.config.js must keep baseUrl at {BASE_PATH!r}")
    for token in (
        "favicon: 'img/aidememo-logo.png'",
        "image: 'img/aidememo-social-card.png'",
        "'docusaurus-pagefind-search'",
    ):
        if token not in config:
            fail(errors, f"website/docusaurus.config.js is missing discovery token {token!r}")


def check_discovery_assets(errors: list[str], pages: dict[str, PageParser]) -> int:
    for locale, locale_config in LOCALES.items():
        route = str(locale_config["base_path"])
        social_url = f"{SITE_ORIGIN}{route}img/aidememo-social-card.png"
        favicon_url = f"{route}img/aidememo-logo.png"
        page = pages.get(route)
        if page is None:
            fail(errors, f"cannot validate discovery metadata because {route} did not render")
            continue
        for key in ("og:image", "twitter:image"):
            if page.metadata.get(key) != social_url:
                fail(errors, f"{route} {key} does not point to {social_url}")
        if page.metadata.get("twitter:card") != "summary_large_image":
            fail(errors, f"{route} must render twitter:card=summary_large_image")
        if ("icon", favicon_url) not in page.links:
            fail(errors, f"{route} is missing favicon link {favicon_url}")
        if locale == "ko" and not re.search(r"[가-힣]", page.metadata.get("description", "")):
            fail(errors, f"{route} metadata description is not localized to Korean")

    social_image = BUILD / "img" / "aidememo-social-card.png"
    if not social_image.exists():
        fail(errors, "build is missing img/aidememo-social-card.png")
    else:
        header = social_image.read_bytes()[:24]
        if len(header) < 24 or header[:8] != b"\x89PNG\r\n\x1a\n":
            fail(errors, "img/aidememo-social-card.png is not a valid PNG")
        else:
            width, height = struct.unpack(">II", header[16:24])
            if (width, height) != (1200, 630):
                fail(errors, f"social card is {width}x{height}; expected 1200x630")

    robots = BUILD / "robots.txt"
    if not robots.exists():
        fail(errors, "build is missing robots.txt")
    else:
        expected_sitemap = f"Sitemap: {SITE_ORIGIN}{BASE_PATH}sitemap.xml"
        if expected_sitemap not in robots.read_text(encoding="utf-8"):
            fail(errors, f"robots.txt is missing {expected_sitemap!r}")

    index_count = 0
    for locale in LOCALES:
        locale_build = BUILD if locale == "en" else BUILD / locale
        pagefind = locale_build / "pagefind"
        language = "en-us" if locale == "en" else "ko-kr"
        if not (locale_build / "pagefind-loader.js").exists():
            fail(errors, f"{locale} build is missing pagefind-loader.js")
        for artifact in ("pagefind.js", "pagefind-worker.js", "pagefind-entry.json"):
            if not (pagefind / artifact).exists():
                fail(errors, f"{locale} Pagefind build is missing {artifact}")
        indexes = sorted((pagefind / "index").glob(f"{language}_*.pf_index"))
        fragments = sorted((pagefind / "fragment").glob(f"{language}_*.pf_fragment"))
        metadata = sorted(pagefind.glob(f"pagefind.{language}_*.pf_meta"))
        if not indexes:
            fail(errors, f"{locale} build produced no Pagefind index for {language}")
        if not fragments:
            fail(errors, f"{locale} build produced no Pagefind fragments for {language}")
        if not metadata:
            fail(errors, f"{locale} build produced no Pagefind metadata for {language}")
        index_count += len(indexes)
    return index_count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="Validate the existing website/build directory instead of running npm build first.",
    )
    args = parser.parse_args()

    if not args.no_build:
        run_build()

    errors: list[str] = []
    check_config(errors)
    for locale in LOCALES:
        sitemap = BUILD / ("sitemap.xml" if locale == "en" else f"{locale}/sitemap.xml")
        if not sitemap.exists():
            fail(
                errors,
                f"{sitemap.relative_to(ROOT)} is missing; run this script without --no-build or run npm build first",
            )
    pages, routes = ({}, [])
    if not errors:
        pages, routes = check_routes(errors)
        link_count, asset_count, anchor_count = check_links_and_assets(errors, pages)
        source_path_count = check_architecture_source_paths(errors)
        search_index_count = check_discovery_assets(errors, pages)
    else:
        link_count = asset_count = anchor_count = source_path_count = search_index_count = 0

    if errors:
        print("docs site e2e failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(
        "docs site e2e passed: "
        f"{len(routes)} pages, {link_count} internal links, {asset_count} assets, "
        f"{anchor_count} anchors, {search_index_count} search indexes, "
        f"{source_path_count} architecture source refs"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
