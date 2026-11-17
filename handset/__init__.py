import re
import sys
import threading
from pathlib import Path
from typing import Optional


def get_bundle_root() -> Path:
    """Return read-only bundle root (supports normal Python & PyInstaller _MEIPASS)."""
    if getattr(sys, "frozen", False):
        return Path(getattr(sys, "_MEIPASS", Path(sys.executable).resolve().parent))
    return Path(__file__).resolve().parent.parent


def get_app_root() -> Path:
    """Return writable application directory (folder of .exe when frozen, repo root otherwise)."""
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent.parent


# Tahap 58: Prefer native C extensions (pylzss / lzfse .pyd) at all times.
# Only append pure-Python _shims at the very end of sys.path if native C import genuinely raises ImportError.
try:
    import lzfse as _native_lzfse  # noqa: F401
    import lzss as _native_lzss  # noqa: F401
except ImportError:
    _shims_candidates = [
        get_bundle_root() / "handset" / "_shims",
        Path(__file__).resolve().parent / "_shims",
    ]
    for _cand in _shims_candidates:
        if _cand.is_dir() and str(_cand) not in sys.path:
            sys.path.append(str(_cand))
            break


_SCAN_RECLAIM_TLS = threading.local()

_IOS_BALLOON_WORDS_PATTERN = (
    r"(?:Auto\s*[-\u2013\u2014]?\s*F[il1I]{1,3}|Isi\s+Otomatis|"
    r"Paste|Tempel|Cut|Potong|Copy|Salin|Look\s+Up|Cari\s+Tahu|"
    r"Translate|Terjemahkan|Share(?:\.\.\.|…)?|Bagikan(?:\.\.\.|…)?|"
    r"Writing\s+Tools|Proofread|Rewrite)"
)
_IOS_BALLOON_FULL_LINE_RE = re.compile(
    rf"^[\s\[\]\(\)\|<>▲▼•·,\-–—]*"
    rf"{_IOS_BALLOON_WORDS_PATTERN}"
    rf"(?:[\s\[\]\(\)\|<>▲▼•·,\-–—]+{_IOS_BALLOON_WORDS_PATTERN})*"
    rf"[\s\[\]\(\)\|<>▲▼•·,\-–—]*$",
    re.IGNORECASE,
)
_IOS_BALLOON_EDGE_STRIP_RE = re.compile(
    rf"(?:^|[\s\[\]\(\)\|<>▲▼•·]+)"
    rf"{_IOS_BALLOON_WORDS_PATTERN}"
    rf"(?:[\s\[\]\(\)\|<>▲▼•·\-–—]+{_IOS_BALLOON_WORDS_PATTERN})*"
    rf"(?=[\s\[\]\(\)\|<>▲▼•·]|$)",
    re.IGNORECASE,
)
_IG_12CHAR_RE = re.compile(
    r"^(https?://(?:www\.)?instagram\.com/(?:p|reels?|tv)/)([A-Za-z0-9_\-]{12})((?:[/?#].*)?)$",
    re.IGNORECASE,
)
_IG_11CHAR_SPLIT_DIGRAMS = (
    ("lt", "T"),
    ("It", "T"),
    ("1t", "T"),
    ("lT", "T"),
    ("IT", "T"),
    ("rn", "m"),
    ("cl", "d"),
    ("cI", "d"),
    ("vv", "w"),
)


def _strip_ios_balloon_tokens_from_line(line: str) -> str:
    """Remove leaked iOS context-menu balloon text (e.g. '[ AutoFill | Paste ]') without hurting URL tokens."""
    if not line:
        return ""
    s = line.strip()
    if not s or _IOS_BALLOON_FULL_LINE_RE.match(s):
        return ""
    cleaned = _IOS_BALLOON_EDGE_STRIP_RE.sub(" ", s)
    cleaned = re.sub(r"[\s\[\]\(\)\|<>▲▼]+$", "", cleaned)
    cleaned = re.sub(r"^[\s\[\]\(\)\|<>▲▼]+(?=(?:https?://|www\.|/|@|[A-Za-z0-9_\-]{8,}))", "", cleaned)
    cleaned = re.sub(r"\s{2,}", " ", cleaned).strip()
    if not cleaned or _IOS_BALLOON_FULL_LINE_RE.match(cleaned):
        return ""
    return cleaned


def _clean_ios_balloon_tokens_from_ocr_lines(lines: list[str]) -> list[str]:
    """Filter out pure iOS context-menu balloon lines and strip edge balloon tokens from OCR lines."""
    out: list[str] = []
    for ln in lines or []:
        c = _strip_ios_balloon_tokens_from_line(str(ln))
        if c:
            out.append(c)
    return out


def _generate_split_glyph_11char_ig_repairs(url: str) -> list[str]:
    """Generate 11-char repaired Instagram URLs when balloon/caret splits a single glyph into 2 chars (12->11)."""
    if not url:
        return []
    m = _IG_12CHAR_RE.match(url.strip())
    if not m:
        return []
    prefix, code, suffix = m.groups()
    if code.endswith("_"):
        return []
    repairs: list[str] = []
    seen: set[str] = set()
    for digram, repl in _IG_11CHAR_SPLIT_DIGRAMS:
        start = 0
        while True:
            idx = code.find(digram, start)
            if idx < 0:
                break
            fixed_code = code[:idx] + repl + code[idx + 2:]
            if len(fixed_code) == 11:
                cand = f"{prefix}{fixed_code}{suffix or '/'}"
                if cand not in seen:
                    seen.add(cand)
                    repairs.append(cand)
            start = idx + 1
    return repairs


def _mask_spotlight_dark_balloon_in_crop(im):
    """Erase dark iOS '[ AutoFill | Paste ]' callout pill & top triangular caret in lower half of light Spotlight crops."""
    try:
        from PIL import Image, ImageDraw
        if im is None:
            return im, False
        w, h = im.size
        if w < 60 or h < 35:
            return im, False

        gray = im.convert("L")
        raw = gray.tobytes()
        gray.close()

        # Guard: apply only on predominantly light-background Spotlight crops (top 42% has >=55% light pixels)
        top_end_y = max(4, int(h * 0.42))
        light_samples = 0
        total_samples = 0
        bright_vals: list[int] = []
        step_x = max(2, w // 40)
        for y in range(2, top_end_y, max(1, top_end_y // 14)):
            row_off = y * w
            for x in range(2, w - 2, step_x):
                v = raw[row_off + x]
                total_samples += 1
                if v >= 165:
                    light_samples += 1
                    bright_vals.append(v)
        if total_samples == 0 or (light_samples / total_samples) < 0.55:
            return im, False

        bright_vals.sort()
        bg_luma = bright_vals[len(bright_vals) // 2] if bright_vals else 245

        y_scan_start = max(int(h * 0.48), 10)
        min_pill_w = max(42, int(w * 0.14))
        dark_thresh = 68

        # Locate horizontal runs of dark pixels per row in lower 52% of crop.
        # Allow tiny internal white-text gaps (up to 10 px inside '[ AutoFill | Paste ]') when merging dark spans.
        row_spans: dict[int, tuple[int, int, float]] = {}
        for y in range(y_scan_start, h):
            row_off = y * w
            row_bytes = raw[row_off : row_off + w]
            # Find leftmost and rightmost dark pixel in longest near-contiguous dark cluster
            best_x0 = -1
            best_x1 = -1
            cur_x0 = -1
            last_dark_x = -1
            dark_cnt = 0
            best_cnt = 0
            max_gap = max(10, int(w * 0.04))
            for x in range(w):
                if row_bytes[x] < dark_thresh:
                    if cur_x0 < 0 or (x - last_dark_x) > max_gap:
                        if cur_x0 >= 0 and (last_dark_x - cur_x0 + 1) >= min_pill_w:
                            span_w = last_dark_x - cur_x0 + 1
                            if span_w > (best_x1 - best_x0 + 1):
                                best_x0, best_x1, best_cnt = cur_x0, last_dark_x, dark_cnt
                        cur_x0 = x
                        dark_cnt = 1
                    else:
                        dark_cnt += 1
                    last_dark_x = x
            if cur_x0 >= 0 and (last_dark_x - cur_x0 + 1) >= min_pill_w:
                span_w = last_dark_x - cur_x0 + 1
                if span_w > (best_x1 - best_x0 + 1):
                    best_x0, best_x1, best_cnt = cur_x0, last_dark_x, dark_cnt
            if best_x0 >= 0:
                span_w = best_x1 - best_x0 + 1
                density = best_cnt / max(1, span_w)
                if density >= 0.58:
                    row_spans[y] = (best_x0, best_x1, density)

        if not row_spans:
            return im, False

        # Group consecutive rows into candidate dark pill clusters
        sorted_ys = sorted(row_spans.keys())
        clusters: list[list[int]] = []
        cur_cluster: list[int] = []
        for y in sorted_ys:
            if not cur_cluster or y <= cur_cluster[-1] + 2:
                cur_cluster.append(y)
            else:
                clusters.append(cur_cluster)
                cur_cluster = [y]
        if cur_cluster:
            clusters.append(cur_cluster)

        min_pill_h = max(6, int(h * 0.06))
        pill_cluster: list[int] | None = None
        for cl in clusters:
            if len(cl) < min_pill_h:
                continue
            solid_cap_rows = sum(1 for yy in cl if row_spans[yy][2] >= 0.84)
            if solid_cap_rows >= 2:
                pill_cluster = cl
                break

        if not pill_cluster:
            return im, False

        y_pill_top = pill_cluster[0]
        y_pill_bot = pill_cluster[-1]
        x0 = min(row_spans[yy][0] for yy in pill_cluster)
        x1 = max(row_spans[yy][1] for yy in pill_cluster)

        work = im.copy()
        if work.mode == "RGB":
            fill_val = (int(bg_luma), int(bg_luma), int(min(255, bg_luma + 2)))
        elif work.mode == "RGBA":
            fill_val = (int(bg_luma), int(bg_luma), int(min(255, bg_luma + 2)), 255)
        else:
            fill_val = int(bg_luma)

        draw = ImageDraw.Draw(work)
        pad_x = 6
        mask_y0 = y_pill_top
        mask_y1 = min(h - 1, y_pill_bot + 4)
        draw.rectangle([max(0, x0 - pad_x), mask_y0, min(w - 1, x1 + pad_x), mask_y1], fill=fill_val)

        # Mask triangular caret immediately above y_pill_top connected to the balloon
        caret_max_h = max(14, int(h * 0.16))
        max_caret_w = max(36, int(w * 0.11))
        prev_run_bounds: tuple[int, int] | None = None
        for step_up in range(1, caret_max_h + 1):
            cy = y_pill_top - step_up
            if cy < int(h * 0.35):
                break
            row_off = cy * w
            # Find short contiguous dark runs within balloon X bounds
            runs: list[tuple[int, int]] = []
            rx0 = -1
            for cx in range(max(0, x0), min(w, x1 + 1)):
                if raw[row_off + cx] < dark_thresh:
                    if rx0 < 0:
                        rx0 = cx
                else:
                    if rx0 >= 0:
                        runs.append((rx0, cx - 1))
                        rx0 = -1
            if rx0 >= 0:
                runs.append((rx0, min(w, x1) - 1 if x1 >= w else x1))
            if not runs:
                break
            matched = False
            for r0, r1 in runs:
                rw = r1 - r0 + 1
                if rw <= max_caret_w and (
                    prev_run_bounds is None
                    or (r0 <= prev_run_bounds[1] + 2 and r1 >= prev_run_bounds[0] - 2 and rw <= (prev_run_bounds[1] - prev_run_bounds[0] + 3))
                ):
                    draw.line([(max(0, r0 - 1), cy), (min(w - 1, r1 + 1), cy)], fill=fill_val)
                    prev_run_bounds = (r0, r1)
                    matched = True
                    break
            if not matched:
                break

        return work, True
    except Exception:
        return im, False


def _invoke_reclaim_memory(trim_hot_ocr: bool = False) -> None:
    fn = getattr(sys, "_iphone_link_reclaim_memory_fn", None)
    if callable(fn):
        try:
            fn(trim_hot_ocr=trim_hot_ocr)
        except Exception:
            pass


def patch_qr_bridge_hot_ocr(qr_module) -> None:
    """Route qr_bridge OCR through persistent Hot WinRT worker + Balloon Immunity & Memory Reclaim (0-diff on qr_bridge.py)."""
    if qr_module is None:
        return

    # 1. Patch _run_winrt_ocr_batch: dark [ AutoFill | Paste ] balloon masking + balloon token filtering + memory cleanup
    orig_fn = getattr(qr_module, "_orig_run_winrt_ocr_batch", None)
    if orig_fn is None:
        orig_fn = getattr(qr_module, "_run_winrt_ocr_batch", None)
        if orig_fn is None:
            return
        qr_module._orig_run_winrt_batch_ref = orig_fn  # compat
        qr_module._orig_run_winrt_ocr_batch = orig_fn

    if not getattr(qr_module._run_winrt_ocr_batch, "_is_hot_ocr_proxy", False):
        def _hot_ocr_winrt_batch(imgs):
            masked_imgs = []
            temp_copies = []
            try:
                for im in (imgs or []):
                    cleaned_im, was_copied = _mask_spotlight_dark_balloon_in_crop(im)
                    masked_imgs.append(cleaned_im)
                    if was_copied and cleaned_im is not None:
                        temp_copies.append(cleaned_im)

                raw_lines_batch = None
                try:
                    from handset.server import _hot_ocr_worker
                    raw_lines_batch = _hot_ocr_worker.run_raw_winrt_batch(masked_imgs)
                except Exception:
                    raw_lines_batch = None

                if raw_lines_batch is None:
                    raw_lines_batch = orig_fn(masked_imgs)

                if isinstance(raw_lines_batch, list):
                    return [
                        _clean_ios_balloon_tokens_from_ocr_lines(frame_lines)
                        for frame_lines in raw_lines_batch
                    ]
                return raw_lines_batch
            finally:
                for tc in temp_copies:
                    try:
                        tc.close()
                    except Exception:
                        pass
                temp_copies.clear()
                masked_imgs.clear()
                if not getattr(sys, "_iphone_link_in_http_scan_handler", False):
                    _invoke_reclaim_memory(trim_hot_ocr=False)

        _hot_ocr_winrt_batch._is_hot_ocr_proxy = True  # type: ignore[attr-defined]
        qr_module._run_winrt_ocr_batch = _hot_ocr_winrt_batch

    # 2. Wrap clean_spotlight_text_line to strip leaked [ AutoFill | Paste ] balloon tokens cleanly
    orig_clean_line = getattr(qr_module, "_orig_clean_spotlight_text_line", None)
    if orig_clean_line is None:
        orig_clean_line = getattr(qr_module, "clean_spotlight_text_line", None)
        if orig_clean_line is not None:
            qr_module._orig_clean_spotlight_text_line = orig_clean_line
    if orig_clean_line is not None and not getattr(qr_module.clean_spotlight_text_line, "_is_tahap214_wrapped", False):
        def _wrapped_clean_spotlight_text_line(line: str) -> str:
            s = _strip_ios_balloon_tokens_from_line(str(line or ""))
            if not s:
                return ""
            return orig_clean_line(s)

        _wrapped_clean_spotlight_text_line._is_tahap214_wrapped = True  # type: ignore[attr-defined]
        qr_module.clean_spotlight_text_line = _wrapped_clean_spotlight_text_line

    # 3. Wrap clean_extracted_url to strip balloon tokens and auto-repair 12-char split-glyph Instagram shortcodes (12 -> 11)
    orig_clean_url = getattr(qr_module, "_orig_clean_extracted_url", None)
    if orig_clean_url is None:
        orig_clean_url = getattr(qr_module, "clean_extracted_url", None)
        if orig_clean_url is not None:
            qr_module._orig_clean_extracted_url = orig_clean_url
    if orig_clean_url is not None and not getattr(qr_module.clean_extracted_url, "_is_tahap214_wrapped", False):
        def _wrapped_clean_extracted_url(raw_url: str, _allow_split: bool = True) -> str:
            pre = _strip_ios_balloon_tokens_from_line(str(raw_url or ""))
            cleaned = orig_clean_url(pre, _allow_split=_allow_split)
            if cleaned:
                repairs = _generate_split_glyph_11char_ig_repairs(cleaned)
                if repairs:
                    return repairs[0]
            return cleaned

        _wrapped_clean_extracted_url._is_tahap214_wrapped = True  # type: ignore[attr-defined]
        qr_module.clean_extracted_url = _wrapped_clean_extracted_url

    # 4. Wrap assemble_multiline_url_from_lines to purge balloon lines & prioritize 11-char repaired candidates
    orig_assemble = getattr(qr_module, "_orig_assemble_multiline_url_from_lines", None)
    if orig_assemble is None:
        orig_assemble = getattr(qr_module, "assemble_multiline_url_from_lines", None)
        if orig_assemble is not None:
            qr_module._orig_assemble_multiline_url_from_lines = orig_assemble
    if orig_assemble is not None and not getattr(qr_module.assemble_multiline_url_from_lines, "_is_tahap214_wrapped", False):
        def _wrapped_assemble_multiline_url_from_lines(lines: list[str]) -> list[str]:
            filtered_lines = _clean_ios_balloon_tokens_from_ocr_lines(lines or [])
            cands = orig_assemble(filtered_lines)
            expanded: list[str] = []
            seen: set[str] = set()
            for c in cands or []:
                for rep in _generate_split_glyph_11char_ig_repairs(c):
                    if rep not in seen:
                        seen.add(rep)
                        expanded.append(rep)
                if c not in seen:
                    seen.add(c)
                    expanded.append(c)
            return expanded

        _wrapped_assemble_multiline_url_from_lines._is_tahap214_wrapped = True  # type: ignore[attr-defined]
        qr_module.assemble_multiline_url_from_lines = _wrapped_assemble_multiline_url_from_lines

    # 5. Wrap pick_best_extracted_url to prefer repaired 11-char codes & demote superseded 12-char split-glyph forms
    orig_pick = getattr(qr_module, "_orig_pick_best_extracted_url", None)
    if orig_pick is None:
        orig_pick = getattr(qr_module, "pick_best_extracted_url", None)
        if orig_pick is not None:
            qr_module._orig_pick_best_extracted_url = orig_pick
    if orig_pick is not None and not getattr(qr_module.pick_best_extracted_url, "_is_tahap214_wrapped", False):
        def _wrapped_pick_best_extracted_url(urls: list[str]) -> Optional[str]:
            expanded: list[str] = []
            superseded_12s: set[str] = set()
            for raw_u in urls or []:
                u_s = _strip_ios_balloon_tokens_from_line(str(raw_u or ""))
                if not u_s:
                    continue
                repairs = _generate_split_glyph_11char_ig_repairs(u_s)
                if repairs:
                    superseded_12s.add(u_s)
                    for rep in repairs:
                        if rep not in expanded:
                            expanded.append(rep)
                else:
                    if u_s not in expanded:
                        expanded.append(u_s)
            if not expanded:
                expanded = [u for u in (urls or []) if u and u not in superseded_12s]
            return orig_pick(expanded)

        _wrapped_pick_best_extracted_url._is_tahap214_wrapped = True  # type: ignore[attr-defined]
        qr_module.pick_best_extracted_url = _wrapped_pick_best_extracted_url

    # 6. Wrap _build_confusable_social_variants to place 11-char split-glyph repairs at top priority (index 0)
    orig_build_vars = getattr(qr_module, "_orig_build_confusable_social_variants", None)
    if orig_build_vars is None:
        orig_build_vars = getattr(qr_module, "_build_confusable_social_variants", None)
        if orig_build_vars is not None:
            qr_module._orig_build_confusable_social_variants = orig_build_vars
    if orig_build_vars is not None and not getattr(qr_module._build_confusable_social_variants, "_is_tahap214_wrapped", False):
        def _wrapped_build_confusable_social_variants(primary_url: str, all_candidates=None, max_variants: int = 6) -> list[str]:
            top_repairs: list[str] = []
            for seed in [primary_url] + list(all_candidates or []):
                for rep in _generate_split_glyph_11char_ig_repairs(str(seed or "")):
                    if rep not in top_repairs:
                        top_repairs.append(rep)
            eff_primary = top_repairs[0] if top_repairs else primary_url
            base_variants = orig_build_vars(eff_primary, all_candidates=all_candidates, max_variants=max_variants)
            merged: list[str] = []
            for u in top_repairs + list(base_variants or []):
                if u and u not in merged:
                    merged.append(u)
            return merged[:max(1, int(max_variants))]

        _wrapped_build_confusable_social_variants._is_tahap214_wrapped = True  # type: ignore[attr-defined]
        qr_module._build_confusable_social_variants = _wrapped_build_confusable_social_variants

    # 7. Wrap top-level QR/URL entrypoints for deterministic scan-end memory reclamation
    for fn_name in (
        "decode_qr_from_image",
        "extract_visible_url_from_frame",
        "extract_qr_or_link_from_frames",
        "extract_url_or_fragment_from_frame",
    ):
        orig_entry = getattr(qr_module, f"_orig_{fn_name}", None)
        if orig_entry is None:
            orig_entry = getattr(qr_module, fn_name, None)
            if orig_entry is None:
                continue
            setattr(qr_module, f"_orig_{fn_name}", orig_entry)
        cur_entry = getattr(qr_module, fn_name, None)
        if getattr(cur_entry, "_is_tahap214_reclaim_wrapped", False):
            continue

        def _make_reclaim_wrapper(target_fn):
            def _wrapped_scan_entry(*args, **kwargs):
                depth = getattr(_SCAN_RECLAIM_TLS, "depth", 0)
                _SCAN_RECLAIM_TLS.depth = depth + 1
                try:
                    return target_fn(*args, **kwargs)
                finally:
                    new_depth = max(0, getattr(_SCAN_RECLAIM_TLS, "depth", 1) - 1)
                    _SCAN_RECLAIM_TLS.depth = new_depth
                    if new_depth == 0 and not getattr(sys, "_iphone_link_in_http_scan_handler", False):
                        _invoke_reclaim_memory(trim_hot_ocr=True)

            _wrapped_scan_entry._is_tahap214_reclaim_wrapped = True  # type: ignore[attr-defined]
            return _wrapped_scan_entry

        setattr(qr_module, fn_name, _make_reclaim_wrapper(orig_entry))


class _QrBridgeHotOcrLoader:
    def __init__(self, orig_loader) -> None:
        self._orig = orig_loader

    def create_module(self, spec):
        if hasattr(self._orig, "create_module"):
            return self._orig.create_module(spec)
        return None

    def exec_module(self, module) -> None:
        self._orig.exec_module(module)
        patch_qr_bridge_hot_ocr(module)

    def __getattr__(self, name: str):
        return getattr(self._orig, name)


class _QrBridgeHotOcrFinder:
    def __init__(self) -> None:
        self._active = False

    def find_spec(self, fullname, path, target=None):
        if fullname != "handset.qr_bridge" or self._active:
            return None
        self._active = True
        try:
            for finder in list(sys.meta_path):
                if isinstance(finder, _QrBridgeHotOcrFinder) or not hasattr(finder, "find_spec"):
                    continue
                spec = finder.find_spec(fullname, path, target)
                if spec is not None and spec.loader is not None and hasattr(spec.loader, "exec_module"):
                    spec.loader = _QrBridgeHotOcrLoader(spec.loader)
                    return spec
        finally:
            self._active = False
        return None


if "handset.qr_bridge" in sys.modules:
    patch_qr_bridge_hot_ocr(sys.modules["handset.qr_bridge"])
if not any(isinstance(f, _QrBridgeHotOcrFinder) for f in sys.meta_path):
    sys.meta_path.insert(0, _QrBridgeHotOcrFinder())
