from __future__ import annotations

import hashlib
import shutil
import sys
import traceback
from dataclasses import replace
from pathlib import Path
from unittest.mock import patch

import pytest
from PIL import Image, ImageDraw

sys.path.insert(0, str(Path(__file__).parents[1]))

from benchmark.run import (  # noqa: E402
    CandidateOutputLimitError,
    RecognitionMeasurement,
    sanitized_candidate_environment,
)
from experiments.table_cells import (  # noqa: E402
    CellRecognition,
    GridRecognition,
    InvalidGridError,
    PageOutputLimitError,
    ProcessLimits,
    TableCleanupError,
    TableRecognitionError,
    build_cell_candidate,
    enforce_page_output_budget,
    is_blank_crop,
    prepare_cell_crop,
    recognize_grid,
    serialize_markdown,
)
from experiments.ruled_table import process_limits  # noqa: E402
from experiments.table_lines import Box, Grid, GridCell  # noqa: E402


SERVICE_ROOT = Path(__file__).parents[1]
CONFIG = SERVICE_ROOT / "experiments" / "ruled-table-config.json"


def limits(**overrides: object) -> ProcessLimits:
    values: dict[str, object] = {
        "cpu_threads": 1,
        "page_timeout_seconds": 20.0,
        "cell_timeout_seconds": 10.0,
        "max_output_bytes_per_cell": 65_536,
        "max_output_bytes_per_page": 1_048_576,
        "max_rss_bytes": 805_306_368,
        "sample_interval_ms": 10,
    }
    values.update(overrides)
    return ProcessLimits(**values)


def rectangular_grid(
    rows: int = 2,
    columns: int = 2,
    *,
    cell_width: int = 100,
    cell_height: int = 50,
) -> Grid:
    table = Box(0, 0, columns * cell_width, rows * cell_height)
    cells = tuple(
        GridCell(
            row=row,
            column=column,
            working_box=Box(
                column * cell_width,
                row * cell_height,
                (column + 1) * cell_width,
                (row + 1) * cell_height,
            ),
            original_box=Box(
                column * cell_width,
                row * cell_height,
                (column + 1) * cell_width,
                (row + 1) * cell_height,
            ),
        )
        for row in range(rows)
        for column in range(columns)
    )
    return Grid(rows, columns, table, table, cells)


def bordered_cell_fixture() -> tuple[Image.Image, Grid]:
    image = Image.new("RGB", (100, 50), "white")
    ImageDraw.Draw(image).rectangle((0, 0, 99, 49), outline="black", width=4)
    return image, rectangular_grid(rows=1, columns=1)


def tessdata_fixture(root: Path) -> Path:
    root.mkdir()
    (root / "vie.traineddata").write_bytes(b"pinned-vie-traineddata")
    return root


def grid_recognition(
    *,
    rows: int,
    columns: int,
    texts: tuple[str, ...],
) -> GridRecognition:
    return GridRecognition(
        rows=rows,
        columns=columns,
        cells=tuple(
            CellRecognition(
                row=index // columns,
                column=index % columns,
                text=text,
                elapsed_seconds=0.0,
                resource={},
            )
            for index, text in enumerate(texts)
        ),
    )


def nonblank_grid_image(grid: Grid) -> Image.Image:
    image = Image.new(
        "L",
        (grid.working_table_box.right, grid.working_table_box.bottom),
        255,
    )
    draw = ImageDraw.Draw(image)
    for cell in grid.cells:
        box = cell.working_box
        draw.rectangle(
            (box.left + 10, box.top + 10, box.left + 20, box.top + 20),
            fill=0,
        )
    return image


def candidate_tessdata(tmp_path: Path) -> Path:
    return tessdata_fixture(tmp_path / "tessdata")


class FakeWorker:
    def __init__(
        self,
        texts: list[str] | None = None,
        *,
        error: BaseException | None = None,
        peak_rss_bytes: int = 1_024,
        cold_peak_rss_bytes: int = 1_024,
    ) -> None:
        self.texts = list(texts or [])
        self.error = error
        self.peak_rss_bytes = peak_rss_bytes
        self.paths: list[Path] = []
        self.timeouts: list[float | None] = []
        self.close_timeouts: list[float | None] = []
        self.close_count = 0
        self.metadata = {
            "cold_initialization": {
                "rss_measurement": {
                    "peak_rss_bytes": cold_peak_rss_bytes,
                }
            }
        }

    def recognize(
        self, page: object, *, timeout_seconds: float | None = None
    ) -> RecognitionMeasurement:
        self.paths.append(page.path)
        self.timeouts.append(timeout_seconds)
        if self.error is not None:
            raise self.error
        return RecognitionMeasurement(
            text=self.texts.pop(0),
            candidate_seconds=0.125,
            resource={
                "method": "sampled_process_tree_rss",
                "peak_rss_bytes": self.peak_rss_bytes,
                "sample_count": 2,
                "sample_interval_seconds": 0.01,
                "wall_seconds": 0.25,
            },
        )

    def close(self, *, timeout_seconds: float | None = None) -> None:
        self.close_count += 1
        self.close_timeouts.append(timeout_seconds)


def recognize_with_worker(
    tmp_path: Path,
    worker: FakeWorker,
    *,
    grid: Grid | None = None,
    image: Image.Image | None = None,
    process_limits: ProcessLimits | None = None,
) -> GridRecognition:
    selected_grid = grid or rectangular_grid()
    owned_image = image is None
    selected_image = image or nonblank_grid_image(selected_grid)
    try:
        with patch(
            "experiments.table_cells._isolated_worker", return_value=worker
        ) as factory:
            result = recognize_grid(
                selected_grid,
                working_image=selected_image,
                candidate_id="balanced-psm6",
                cell_inset_pixels=4,
                psm=6,
                tessdata=candidate_tessdata(tmp_path),
                limits=process_limits or limits(),
            )
        factory.assert_called_once()
        return result
    finally:
        if owned_image:
            selected_image.close()


def test_crop_insets_and_erases_residual_border() -> None:
    image, grid = bordered_cell_fixture()
    try:
        crop = prepare_cell_crop(image, grid.cells[0], inset_pixels=4)
        try:
            assert crop.mode == "L"
            assert crop.size == (92, 42)
            assert min(crop.getpixel((x, 0)) for x in range(crop.width)) == 255
            assert min(crop.getpixel((x, 1)) for x in range(crop.width)) == 255
            assert min(crop.getpixel((0, y)) for y in range(crop.height)) == 255
        finally:
            crop.close()
    finally:
        image.close()


def test_crop_is_detached_from_source_image() -> None:
    image, grid = bordered_cell_fixture()
    crop = prepare_cell_crop(image, grid.cells[0], inset_pixels=4)
    image.close()
    try:
        assert crop.getpixel((2, 2)) == 255
    finally:
        crop.close()


@pytest.mark.parametrize("inset", [25, 26])
def test_crop_rejects_nonpositive_interior(inset: int) -> None:
    image, grid = bordered_cell_fixture()
    try:
        with pytest.raises(ValueError, match="positive interior"):
            prepare_cell_crop(image, grid.cells[0], inset_pixels=inset)
    finally:
        image.close()


def test_crop_rejects_cell_outside_working_image() -> None:
    image = Image.new("L", (99, 50), 255)
    grid = rectangular_grid(rows=1, columns=1)
    try:
        with pytest.raises(ValueError, match="working image"):
            prepare_cell_crop(image, grid.cells[0], inset_pixels=4)
    finally:
        image.close()


def test_blank_detection_uses_exact_99_point_5_percent_threshold() -> None:
    crop = Image.new("L", (20, 20), 255)
    try:
        crop.putpixel((5, 5), 0)
        crop.putpixel((6, 5), 0)
        assert is_blank_crop(crop)
        crop.putpixel((7, 5), 0)
        assert not is_blank_crop(crop)
    finally:
        crop.close()


def test_blank_cells_skip_worker_and_have_zero_subprocess_time(
    tmp_path: Path,
) -> None:
    grid = rectangular_grid()
    image = Image.new("L", (200, 100), 255)
    worker = FakeWorker([])
    try:
        recognition = recognize_with_worker(
            tmp_path, worker, grid=grid, image=image
        )
    finally:
        image.close()
    assert worker.paths == []
    assert [cell.text for cell in recognition.cells] == ["", "", "", ""]
    assert [cell.elapsed_seconds for cell in recognition.cells] == [
        0.0,
        0.0,
        0.0,
        0.0,
    ]
    assert worker.close_count == 1


@pytest.mark.parametrize("psm", [6, 7])
def test_builds_exact_tesseract_vie_candidate(
    tmp_path: Path, psm: int
) -> None:
    tessdata = candidate_tessdata(tmp_path)
    spec = build_cell_candidate(
        candidate_id=f"candidate-{psm}",
        psm=psm,
        tessdata=tessdata,
        limits=limits(),
    )
    expected_hash = hashlib.sha256(
        (tessdata / "vie.traineddata").read_bytes()
    ).hexdigest()
    assert spec.argv == (
        "tesseract",
        "{input}",
        "stdout",
        "-l",
        "vie",
        "--psm",
        str(psm),
    )
    assert dict(spec.environment) == {
        **sanitized_candidate_environment(cpu_threads=1),
        "TESSDATA_PREFIX": str(tessdata),
    }
    assert dict(spec.provenance) == {
        "engine": "tesseract-cli",
        "langs": "vie",
        "psm": psm,
        "tessdata_sha256": expected_hash,
    }


@pytest.mark.parametrize("psm", [0, 3, 8, True])
def test_cell_psm_allowlist_is_only_six_and_seven(
    tmp_path: Path, psm: int
) -> None:
    with pytest.raises(ValueError, match="PSM must be 6 or 7"):
        build_cell_candidate(
            candidate_id="invalid",
            psm=psm,
            tessdata=candidate_tessdata(tmp_path),
            limits=limits(),
        )


def test_candidate_requires_local_tessdata_with_vie_model(tmp_path: Path) -> None:
    remote_like = Path("https://example.invalid/tessdata")
    with pytest.raises(ValueError, match="local"):
        build_cell_candidate(
            candidate_id="remote",
            psm=6,
            tessdata=remote_like,
            limits=limits(),
        )
    empty = tmp_path / "empty"
    empty.mkdir()
    with pytest.raises(ValueError, match="vie.traineddata"):
        build_cell_candidate(
            candidate_id="missing-model",
            psm=6,
            tessdata=empty,
            limits=limits(),
        )


def test_process_limits_are_loaded_from_validated_canonical_config() -> None:
    loaded = process_limits(CONFIG)
    assert loaded == limits()


def test_process_limits_cannot_exceed_canonical_resource_caps() -> None:
    with pytest.raises(ValueError, match="max_output_bytes_per_cell"):
        replace(limits(), max_output_bytes_per_cell=65_537)
    with pytest.raises(ValueError, match="max_output_bytes_per_page"):
        replace(limits(), max_output_bytes_per_page=1_048_577)
    with pytest.raises(ValueError, match="max_rss_bytes"):
        replace(limits(), max_rss_bytes=805_306_369)


def test_grid_recognition_requires_complete_unique_row_major_cells() -> None:
    cell = CellRecognition(0, 0, "", 0.0, {})
    with pytest.raises(ValueError, match="complete.*row-major"):
        GridRecognition(rows=2, columns=2, cells=(cell,))
    duplicate = tuple(
        CellRecognition(0, 0, "", 0.0, {}) for _ in range(4)
    )
    with pytest.raises(ValueError, match="complete.*row-major"):
        GridRecognition(rows=2, columns=2, cells=duplicate)


def test_grid_recognition_caps_cells_at_1500() -> None:
    cells = tuple(
        CellRecognition(
            row=index // 30,
            column=index % 30,
            text="",
            elapsed_seconds=0.0,
            resource={},
        )
        for index in range(1_501)
    )
    with pytest.raises(ValueError, match="1500"):
        GridRecognition(rows=50, columns=30, cells=cells)


@pytest.mark.parametrize(
    ("rows", "columns", "expected"),
    [
        (51, 1, "rows"),
        (1, 31, "columns"),
    ],
)
def test_grid_recognition_enforces_exact_dimension_caps(
    rows: int,
    columns: int,
    expected: str,
) -> None:
    cells = tuple(
        CellRecognition(
            row=index // columns,
            column=index % columns,
            text="",
            elapsed_seconds=0.0,
            resource={},
        )
        for index in range(rows * columns)
    )
    with pytest.raises(ValueError, match=expected):
        GridRecognition(rows=rows, columns=columns, cells=cells)


def test_cell_resources_are_ignored_by_repr_and_equality() -> None:
    left = CellRecognition(0, 0, "PRIVATE TEXT", 1.0, {"peak_rss_bytes": 1})
    right = CellRecognition(0, 0, "PRIVATE TEXT", 1.0, {"peak_rss_bytes": 999})
    assert left == right
    assert "peak_rss_bytes" not in repr(left)
    assert "PRIVATE TEXT" not in repr(left)


def test_one_worker_recognizes_nonblank_cells_row_major_with_numeric_paths(
    tmp_path: Path,
) -> None:
    worker = FakeWorker(["A", "B", "C", "D"])
    recognition = recognize_with_worker(tmp_path, worker)
    assert [(cell.row, cell.column) for cell in recognition.cells] == [
        (0, 0),
        (0, 1),
        (1, 0),
        (1, 1),
    ]
    assert [cell.text for cell in recognition.cells] == ["A", "B", "C", "D"]
    assert [cell.elapsed_seconds for cell in recognition.cells] == [
        0.25,
        0.25,
        0.25,
        0.25,
    ]
    assert [path.name for path in worker.paths] == [
        "cell-0000-0000.png",
        "cell-0000-0001.png",
        "cell-0001-0000.png",
        "cell-0001-0001.png",
    ]
    assert worker.close_count == 1


def test_output_budget_is_additive_across_cells() -> None:
    with pytest.raises(PageOutputLimitError):
        enforce_page_output_budget([40_000, 40_000], maximum=65_536)


def test_output_budgets_count_utf8_bytes_not_python_characters(
    tmp_path: Path,
) -> None:
    grid = rectangular_grid(rows=1, columns=1)
    worker = FakeWorker(["ệ" * 30_000])
    with pytest.raises(TableRecognitionError) as caught:
        recognize_with_worker(
            tmp_path,
            worker,
            grid=grid,
            process_limits=limits(max_output_bytes_per_cell=65_536),
        )
    assert caught.value.error_kind == "output_limit"
    assert worker.close_count == 1


def test_page_output_budget_is_enforced_during_grid_recognition(
    tmp_path: Path,
) -> None:
    worker = FakeWorker(["a" * 40_000, "b" * 40_000, "c", "d"])
    with pytest.raises(TableRecognitionError) as caught:
        recognize_with_worker(
            tmp_path,
            worker,
            process_limits=limits(max_output_bytes_per_page=65_536),
        )
    assert caught.value.error_kind == "output_limit"
    assert len(worker.paths) == 2
    assert worker.close_count == 1


@pytest.mark.parametrize(
    ("failure", "expected_kind"),
    [
        (TimeoutError("PRIVATE_TIMEOUT_CANARY"), "timeout"),
        (CandidateOutputLimitError("PRIVATE_OUTPUT_CANARY"), "output_limit"),
        (RuntimeError("PRIVATE_FAILURE_CANARY"), "candidate_error"),
    ],
)
def test_worker_failures_are_typed_and_sanitized(
    tmp_path: Path,
    failure: BaseException,
    expected_kind: str,
) -> None:
    worker = FakeWorker(error=failure)
    with pytest.raises(TableRecognitionError) as caught:
        recognize_with_worker(tmp_path, worker)
    assert caught.value.error_kind == expected_kind
    assert "CANARY" not in str(caught.value)
    assert worker.close_count == 1


def test_rss_must_be_strictly_below_bound(tmp_path: Path) -> None:
    worker = FakeWorker(["A"], peak_rss_bytes=805_306_368)
    grid = rectangular_grid(rows=1, columns=1)
    with pytest.raises(TableRecognitionError) as caught:
        recognize_with_worker(tmp_path, worker, grid=grid)
    assert caught.value.error_kind == "resource_limit"
    assert worker.close_count == 1


def test_cold_worker_rss_must_be_strictly_below_bound(tmp_path: Path) -> None:
    worker = FakeWorker(
        ["A"],
        cold_peak_rss_bytes=805_306_368,
    )
    grid = rectangular_grid(rows=1, columns=1)
    with pytest.raises(TableRecognitionError) as caught:
        recognize_with_worker(tmp_path, worker, grid=grid)
    assert caught.value.error_kind == "resource_limit"
    assert worker.paths == []
    assert worker.close_count == 1


@pytest.mark.parametrize(
    ("startup_failure", "expected_kind"),
    [
        (TimeoutError("PRIVATE_STARTUP_TIMEOUT"), "timeout"),
        (RuntimeError("PRIVATE_STARTUP_FAILURE"), "candidate_error"),
    ],
)
def test_worker_startup_failures_are_typed_sanitized_and_cleaned(
    tmp_path: Path,
    startup_failure: BaseException,
    expected_kind: str,
) -> None:
    page_directory = tmp_path / "startup-page"
    page_directory.mkdir()
    grid = rectangular_grid(rows=1, columns=1)
    image = nonblank_grid_image(grid)
    try:
        with (
            patch(
                "experiments.table_cells.tempfile.mkdtemp",
                return_value=str(page_directory),
            ),
            patch(
                "experiments.table_cells._isolated_worker",
                side_effect=startup_failure,
            ),
            pytest.raises(TableRecognitionError) as caught,
        ):
            recognize_grid(
                grid,
                working_image=image,
                candidate_id="balanced-psm6",
                cell_inset_pixels=4,
                psm=6,
                tessdata=candidate_tessdata(tmp_path),
                limits=limits(),
            )
    finally:
        image.close()
    assert caught.value.error_kind == expected_kind
    assert "PRIVATE" not in str(caught.value)
    assert not page_directory.exists()


def test_page_deadline_is_checked_after_each_response(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class Clock:
        now = 0.0

        def monotonic(self) -> float:
            return self.now

    class DelayedResponseWorker(FakeWorker):
        def recognize(
            self, page: object, *, timeout_seconds: float | None = None
        ) -> RecognitionMeasurement:
            measurement = super().recognize(
                page,
                timeout_seconds=timeout_seconds,
            )
            clock.now = 21.0
            return measurement

    clock = Clock()
    monkeypatch.setattr(
        "experiments.table_cells.time.monotonic", clock.monotonic
    )
    worker = DelayedResponseWorker(["A"])
    grid = rectangular_grid(rows=1, columns=1)
    with pytest.raises(TableRecognitionError) as caught:
        recognize_with_worker(tmp_path, worker, grid=grid)
    assert caught.value.error_kind == "timeout"
    assert worker.close_count == 1


def test_all_blank_page_times_out_during_preprocessing_without_sleep(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class Clock:
        now = 0.0

        def monotonic(self) -> float:
            return self.now

    clock = Clock()
    monkeypatch.setattr(
        "experiments.table_cells.time.monotonic", clock.monotonic
    )
    worker = FakeWorker([])
    grid = rectangular_grid()
    image = Image.new("L", (200, 100), 255)

    def delayed_blank(_crop: Image.Image) -> bool:
        clock.now = 21.0
        return True

    try:
        with (
            patch(
                "experiments.table_cells.is_blank_crop",
                side_effect=delayed_blank,
            ),
            pytest.raises(TableRecognitionError) as caught,
        ):
            recognize_with_worker(
                tmp_path,
                worker,
                grid=grid,
                image=image,
            )
    finally:
        image.close()
    assert caught.value.error_kind == "timeout"
    assert worker.paths == []
    assert worker.close_count == 1


def test_page_deadline_is_enforced_during_tessdata_hashing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ticks = iter((0.0, 0.0, 0.0, 21.0))
    monkeypatch.setattr(
        "experiments.table_cells.time.monotonic", lambda: next(ticks)
    )
    grid = rectangular_grid(rows=1, columns=1)
    image = nonblank_grid_image(grid)
    try:
        with pytest.raises(TableRecognitionError) as caught:
            recognize_grid(
                grid,
                working_image=image,
                candidate_id="hash-timeout",
                cell_inset_pixels=4,
                psm=6,
                tessdata=candidate_tessdata(tmp_path),
                limits=limits(),
            )
    finally:
        image.close()
    assert caught.value.error_kind == "timeout"


def test_candidate_setup_failure_has_no_private_traceback_context(
    tmp_path: Path,
) -> None:
    canary = f"PRIVATE_SETUP_CANARY:{tmp_path}"
    grid = rectangular_grid(rows=1, columns=1)
    image = nonblank_grid_image(grid)
    try:
        with (
            patch(
                "experiments.table_cells.hash_vie_traineddata",
                side_effect=RuntimeError(canary),
            ),
            pytest.raises(TableRecognitionError) as caught,
        ):
            recognize_grid(
                grid,
                working_image=image,
                candidate_id="setup-failure",
                cell_inset_pixels=4,
                psm=6,
                tessdata=tmp_path / "private-tessdata",
                limits=limits(),
            )
    finally:
        image.close()
    formatted = "".join(
        traceback.format_exception(
            type(caught.value),
            caught.value,
            caught.value.__traceback__,
        )
    )
    assert caught.value.error_kind == "candidate_error"
    assert canary not in formatted
    assert str(tmp_path) not in formatted


def test_remaining_page_deadline_is_passed_to_worker(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class Clock:
        now = 0.0

        def monotonic(self) -> float:
            return self.now

    clock = Clock()
    monkeypatch.setattr(
        "experiments.table_cells.time.monotonic", clock.monotonic
    )
    worker = FakeWorker(["A"])
    grid = rectangular_grid(rows=1, columns=1)
    with patch(
        "experiments.table_cells.is_blank_crop",
        side_effect=lambda _crop: setattr(clock, "now", 4.0) or False,
    ):
        recognize_with_worker(
            tmp_path,
            worker,
            grid=grid,
            process_limits=limits(
                page_timeout_seconds=5.0, cell_timeout_seconds=10.0
            ),
        )
    assert worker.timeouts == [1.0]


def test_crops_and_owned_page_directory_are_removed_on_failure(
    tmp_path: Path,
) -> None:
    page_directory = tmp_path / "owned-page"
    page_directory.mkdir()
    worker = FakeWorker(error=RuntimeError("failure"))
    grid = rectangular_grid(rows=1, columns=1)
    image = nonblank_grid_image(grid)
    try:
        with (
            patch(
                "experiments.table_cells.tempfile.mkdtemp",
                return_value=str(page_directory),
            ),
            patch(
                "experiments.table_cells._isolated_worker",
                return_value=worker,
            ),
            pytest.raises(TableRecognitionError),
        ):
            recognize_grid(
                grid,
                working_image=image,
                candidate_id="balanced-psm6",
                cell_inset_pixels=4,
                psm=6,
                tessdata=candidate_tessdata(tmp_path),
                limits=limits(),
            )
    finally:
        image.close()
    assert worker.close_count == 1
    assert not page_directory.exists()


def test_primary_failure_is_preserved_and_close_fault_is_reported_safely(
    tmp_path: Path,
) -> None:
    class CloseFaultWorker(FakeWorker):
        def close(self, *, timeout_seconds: float | None = None) -> None:
            self.close_count += 1
            self.close_timeouts.append(timeout_seconds)
            if self.close_count == 1:
                raise RuntimeError(f"PRIVATE_CLOSE_CANARY:{tmp_path}")

    page_directory = tmp_path / "close-fault-page"
    page_directory.mkdir()
    worker = CloseFaultWorker(error=RuntimeError("PRIVATE_PRIMARY_CANARY"))
    grid = rectangular_grid(rows=1, columns=1)
    image = nonblank_grid_image(grid)
    try:
        with (
            patch(
                "experiments.table_cells.tempfile.mkdtemp",
                return_value=str(page_directory),
            ),
            patch(
                "experiments.table_cells._isolated_worker",
                return_value=worker,
            ),
            pytest.raises(TableRecognitionError) as caught,
        ):
            recognize_grid(
                grid,
                working_image=image,
                candidate_id="close-fault",
                cell_inset_pixels=4,
                psm=6,
                tessdata=candidate_tessdata(tmp_path),
                limits=limits(),
            )
    finally:
        image.close()
    formatted = "".join(
        traceback.format_exception(
            type(caught.value),
            caught.value,
            caught.value.__traceback__,
        )
    )
    assert caught.value.error_kind == "candidate_error"
    assert isinstance(caught.value.cleanup_failure, TableCleanupError)
    assert "PRIVATE" not in formatted
    assert str(tmp_path) not in formatted
    assert worker.close_count == 2
    assert not page_directory.exists()


def test_recursive_temp_cleanup_fault_is_retried_reported_and_leak_free(
    tmp_path: Path,
) -> None:
    page_directory = tmp_path / "rmtree-fault-page"
    page_directory.mkdir()
    worker = FakeWorker([])
    grid = rectangular_grid(rows=1, columns=1)
    image = Image.new("L", (100, 50), 255)
    real_rmtree = shutil.rmtree
    calls = 0

    def faulty_rmtree(path: Path) -> None:
        nonlocal calls
        calls += 1
        if calls == 1:
            raise OSError(f"PRIVATE_RMTREE_CANARY:{path}")
        real_rmtree(path)

    try:
        with (
            patch(
                "experiments.table_cells.tempfile.mkdtemp",
                return_value=str(page_directory),
            ),
            patch(
                "experiments.table_cells._isolated_worker",
                return_value=worker,
            ),
            patch(
                "experiments.table_cells.shutil.rmtree",
                side_effect=faulty_rmtree,
            ),
            pytest.raises(TableCleanupError) as caught,
        ):
            recognize_grid(
                grid,
                working_image=image,
                candidate_id="rmtree-fault",
                cell_inset_pixels=4,
                psm=6,
                tessdata=candidate_tessdata(tmp_path),
                limits=limits(),
            )
    finally:
        image.close()
    formatted = "".join(
        traceback.format_exception(
            type(caught.value),
            caught.value,
            caught.value.__traceback__,
        )
    )
    assert caught.value.error_kind == "cleanup_error"
    assert "PRIVATE" not in formatted
    assert str(tmp_path) not in formatted
    assert calls == 2
    assert not page_directory.exists()


def test_cleanup_time_counts_toward_page_deadline(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class Clock:
        now = 0.0

        def monotonic(self) -> float:
            return self.now

    class DelayedCloseWorker(FakeWorker):
        def close(self, *, timeout_seconds: float | None = None) -> None:
            self.close_count += 1
            self.close_timeouts.append(timeout_seconds)
            clock.now = 21.0

    clock = Clock()
    monkeypatch.setattr(
        "experiments.table_cells.time.monotonic", clock.monotonic
    )
    worker = DelayedCloseWorker([])
    grid = rectangular_grid(rows=1, columns=1)
    image = Image.new("L", (100, 50), 255)
    try:
        with pytest.raises(TableRecognitionError) as caught:
            recognize_with_worker(
                tmp_path,
                worker,
                grid=grid,
                image=image,
            )
    finally:
        image.close()
    assert caught.value.error_kind == "timeout"
    assert worker.close_count == 1
    assert worker.close_timeouts == [20.0]


def test_invalid_cell_crop_maps_to_sanitized_invalid_grid(
    tmp_path: Path,
) -> None:
    grid = rectangular_grid(rows=1, columns=1)
    image = Image.new("L", (99, 50), 255)
    worker = FakeWorker([])
    try:
        with pytest.raises(TableRecognitionError) as caught:
            recognize_with_worker(
                tmp_path,
                worker,
                grid=grid,
                image=image,
            )
    finally:
        image.close()
    assert caught.value.error_kind == "invalid_grid"
    assert worker.close_count == 1


def test_normalization_changes_only_nfc_and_whitespace(
    tmp_path: Path,
) -> None:
    worker = FakeWorker(["  Ma\u0303 | số  \r\n\t01—Đ  \r  "])
    grid = rectangular_grid(rows=1, columns=1)
    recognition = recognize_with_worker(tmp_path, worker, grid=grid)
    assert recognition.cells[0].text == "Mã | số\n01—Đ"


def test_markdown_escapes_pipes_and_preserves_multiline_cells() -> None:
    recognition = grid_recognition(
        rows=2,
        columns=2,
        texts=("Mã | code", "Giá trị", "01\nA", ""),
    )
    assert serialize_markdown(recognition) == (
        "| Mã \\| code | Giá trị |\n"
        "|---|---|\n"
        "| 01<br>A |  |\n"
    )


def test_markdown_escapes_backslashes_before_pipes() -> None:
    recognition = grid_recognition(
        rows=2,
        columns=2,
        texts=(r"A\B | C", "D", "E", "F"),
    )
    assert serialize_markdown(recognition).splitlines()[0] == (
        r"| A\\B \| C | D |"
    )


def test_markdown_rejects_grid_smaller_than_two_by_two() -> None:
    recognition = grid_recognition(rows=1, columns=2, texts=("A", "B"))
    with pytest.raises(InvalidGridError) as caught:
        serialize_markdown(recognition)
    assert caught.value.error_kind == "invalid_grid"


def test_serializer_revalidates_exact_grid_bounds_and_cell_count() -> None:
    recognition = grid_recognition(
        rows=2,
        columns=2,
        texts=("A", "B", "C", "D"),
    )
    object.__setattr__(recognition, "rows", 51)
    with pytest.raises(InvalidGridError):
        serialize_markdown(recognition)

    object.__setattr__(recognition, "rows", 2)
    object.__setattr__(recognition, "cells", recognition.cells[:-1])
    with pytest.raises(InvalidGridError):
        serialize_markdown(recognition)


def test_candidate_execution_never_uses_shell(tmp_path: Path) -> None:
    worker = FakeWorker(["A"])
    grid = rectangular_grid(rows=1, columns=1)
    with patch("subprocess.run") as run, patch("subprocess.call") as call:
        recognize_with_worker(tmp_path, worker, grid=grid)
    run.assert_not_called()
    call.assert_not_called()
