"""python-chess (github.com/niklasf/python-chess, GPL-3.0) + Stockfish
(github.com/official-stockfish/Stockfish, GPL-3.0) -> CuaTask converter for
the held-out-only `chess` family.

## The game as a screen, and the controller-vs-discrete-options decision

A cursor-based game-controller framing of chess (arrow keys move a
highlighted cursor square by square, one button lifts/drops a piece, and the
model composes a *sequence* of key presses to make one move) is a genuinely
different, harder task shape than every other family in this benchmark
(CuaTask scores ONE decision per state, not a multi-step key sequence to
reach a decision).

This integration deliberately takes the simpler alternative: each rendered
position is scored as ONE one-pass decision over the position's own real
legal moves (each legal move is a candidate "element" a model picks or
skips), with Stockfish's own top move as gold. This is a real simplification
versus a controller framing -- it skips the "can the model operate a
cursor+lift/drop controller at all" sub-skill entirely. What this integration
keeps genuine: real chess positions, a real board render, a real Stockfish
gold move, and a real one-pass scoreable decision -- CuaTask's own native
shape.

## Modality decision: BOTH multimodal and text are honestly answerable

Chess is well suited to both modalities because FEN notation is a real,
complete, standard textual encoding of a chess position (piece placement,
side to move, castling rights, en-passant square) -- not a synthesized
stand-in. A model given a task's FEN and its legal-move list has no less real
information than a model given the rendered board image (arguably more,
since reading piece identity/position off a small rendered PNG is itself an
error-prone perception step FEN skips entirely). So both modalities are
offered here, honestly, each fed only its own real representation:
  - multimodal: the rendered board PNG only (no FEN in the prompt).
  - text: the FEN + side-to-move + legal-move list as `ax_tree` (no image).

This family is held out only: it is never mixed into the synthetic/real GUI
families used for training, only used to measure generalization to a chess
domain a GUI-trained model has never seen.

If Stockfish is not available on PATH, `_fallback_gold_move` (plain
legal-move selection, no engine) is used instead and disclosed via
`provenance["gold_label_method"] = "fallback_first_legal_move"` -- a
genuinely weaker oracle, never silently substituted for a real Stockfish
label.
"""
from __future__ import annotations

import random
from dataclasses import dataclass
from pathlib import Path

from ..task import CuaTask, OptionSpec

SQ = 40  # pixel size of one square in the rendered board
BOARD_PX = SQ * 8
LIGHT = (240, 217, 181)
DARK = (181, 136, 99)
WHITE_PIECE = (250, 250, 250)
BLACK_PIECE = (20, 20, 20)
OUTLINE = (0, 0, 0)

_PIECE_LETTERS = {
    "P": "P", "N": "N", "B": "B", "R": "R", "Q": "Q", "K": "K",
    "p": "P", "n": "N", "b": "B", "r": "R", "q": "Q", "k": "K",
}


def render_board_png(board, path: Path) -> None:
    """Renders `board` (a `chess.Board`) as a plain, legible BOARD_PX x
    BOARD_PX PNG: alternating light/dark squares, pieces drawn as filled
    circles with their letter (uppercase=white, lowercase=black per FEN
    convention, rendered here as white-fill/black-fill circles with a black
    letter). Always drawn from White's own orientation (rank 8 at the top) --
    a real, disclosed simplification (there is no controller/cursor at all,
    so there is no single-perspective-per-turn requirement driving a flip).
    No cursor, no highlight, no legal-move dots are drawn -- doing so would
    require picking a "selected" square, and any such selection would leak
    information about which move this task considers gold. The rendered
    image is deliberately gold-blind.
    """
    from PIL import Image, ImageDraw, ImageFont

    import chess

    img = Image.new("RGB", (BOARD_PX, BOARD_PX), LIGHT)
    draw = ImageDraw.Draw(img)
    try:
        font = ImageFont.truetype("arial.ttf", 22)
    except Exception:
        font = ImageFont.load_default()

    for rank in range(8):  # rank 0 = rank 8 (top row) in this top-down draw
        for file in range(8):
            x0, y0 = file * SQ, rank * SQ
            color = LIGHT if (rank + file) % 2 == 0 else DARK
            draw.rectangle([x0, y0, x0 + SQ, y0 + SQ], fill=color)

    for square in chess.SQUARES:
        piece = board.piece_at(square)
        if piece is None:
            continue
        file = chess.square_file(square)
        rank = 7 - chess.square_rank(square)  # square_rank 7 == rank 8 == top row (rank index 0)
        cx, cy = file * SQ + SQ // 2, rank * SQ + SQ // 2
        r = SQ // 2 - 4
        fill = WHITE_PIECE if piece.color == chess.WHITE else BLACK_PIECE
        text_color = (0, 0, 0) if piece.color == chess.WHITE else (255, 255, 255)
        draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=fill, outline=OUTLINE, width=2)
        letter = _PIECE_LETTERS[piece.symbol()]
        bbox = draw.textbbox((0, 0), letter, font=font)
        tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
        draw.text((cx - tw / 2 - bbox[0], cy - th / 2 - bbox[1]), letter, fill=text_color, font=font)

    Path(path).parent.mkdir(parents=True, exist_ok=True)
    img.save(path)


def square_pixel_frame(square: int) -> list[int]:
    """Real pixel bounding box (in `render_board_png`'s own coordinate
    system, White's-perspective top-down) of one square -- used as the
    synthetic element's `frame` so it points at an actually-visible region
    of the rendered image, not an arbitrary placeholder."""
    import chess

    file = chess.square_file(square)
    rank = 7 - chess.square_rank(square)
    x0, y0 = file * SQ, rank * SQ
    return [x0, y0, x0 + SQ, y0 + SQ]


def fen_ax_tree(board) -> str:
    """The real, complete FEN-derived text state for the text modality: FEN
    string, side to move, castling rights, and the position's own legal-move
    list in UCI notation (the same options the task actually scores) -- no
    indication of which is gold."""
    import chess

    legal = sorted(board.san(m) for m in board.legal_moves)
    return (
        f"FEN: {board.fen()}\n"
        f"Side to move: {'white' if board.turn == chess.WHITE else 'black'}\n"
        f"Legal moves (SAN): {', '.join(legal)}"
    )


def _fallback_gold_move(board):
    """No-Stockfish fallback: python-chess's own legal-move generator has no
    notion of move quality, so this picks the FIRST legal move in
    python-chess's own deterministic (seed-independent) move order. This is
    a genuinely weaker oracle than Stockfish (not "chess-aware" at all) --
    used only when Stockfish truly is not on PATH, and always disclosed via
    `provenance["gold_label_method"] = "fallback_first_legal_move"` so a
    dataset built this way is never confused with a real Stockfish-taught
    one."""
    return next(iter(board.legal_moves))


@dataclass
class ChessPosition:
    board: "object"          # a chess.Board
    gold_move: "object"      # a chess.Move
    gold_method: str          # "stockfish_depth<N>" | "fallback_first_legal_move"


def gold_move_for(board, engine=None, depth: int = 10) -> ChessPosition:
    if engine is not None:
        import chess.engine

        result = engine.play(board, chess.engine.Limit(depth=depth))
        return ChessPosition(board=board, gold_move=result.move, gold_method=f"stockfish_depth{depth}")
    return ChessPosition(board=board, gold_move=_fallback_gold_move(board), gold_method="fallback_first_legal_move")


def convert_position(
    *,
    position: ChessPosition,
    game_idx: int,
    ply: int,
    out_dir: Path,
    modality_available: tuple[str, ...] = ("multimodal", "text"),
) -> CuaTask | None:
    """Converts one `ChessPosition` into a CuaTask. Returns None if the
    position has no legal moves (game already over -- checkmate/stalemate;
    an honestly unanswerable, not fabricated, state)."""
    import chess

    board = position.board
    legal_moves = list(board.legal_moves)
    if not legal_moves:
        return None

    task_id = f"chess_{game_idx}_{ply}"
    elements, options, expected = [], [], {}
    for i, move in enumerate(legal_moves):
        eid = f"el_{i}"
        san = board.san(move)
        frame = square_pixel_frame(move.to_square)
        elements.append({"id": eid, "role": "Button", "label": f"{move.uci()} ({san})", "frame": frame})
        is_gold = move == position.gold_move
        # Hard-distractor fix: EVERY legal move gets a real {click, skip}
        # option pair (click = "play this move"), not just the gold move.
        # Previously every non-gold element was skip-only (a single-option
        # categorical, trivially "correct" under softmax), so the task
        # reduced to "spot the one element with 2 options" instead of real
        # move selection over the full legal-move set. Now the model must
        # discriminate the single gold move against every other legal move
        # as a real, plausible click candidate -- the same hard-distractor
        # principle the GUI families use (see README's "Data sources and
        # provenance" section), applied here per-move instead of per-decoy.
        options.append(OptionSpec(element_id=eid, role="Button", label=f"{move.uci()} ({san})", action="click"))
        options.append(OptionSpec(element_id=eid, role="Button", label=f"{move.uci()} ({san})", action="skip"))
        expected[eid] = "click" if is_gold else "skip"

    screenshot_rel = None
    ax_tree = None
    if "multimodal" in modality_available:
        out_dir.mkdir(parents=True, exist_ok=True)
        shot_path = out_dir / f"{task_id}.png"
        render_board_png(board, shot_path)
        screenshot_rel = str(shot_path)
    if "text" in modality_available:
        ax_tree = fen_ax_tree(board)

    return CuaTask(
        id=task_id,
        family="chess",
        app="chess",
        modality_available=list(modality_available),
        screenshot=screenshot_rel,
        ax_tree=ax_tree,
        ax_tree_source="synthetic" if ax_tree else None,  # a FEN-derived text rendering, not a captured a11y tree
        elements=elements,
        elements_source="synthetic_spec",
        entities=[],
        options=options,
        expected=expected,
        split="public",
        group=None,
        provenance={
            "source": "python-chess + Stockfish",
            "source_url": "https://github.com/niklasf/python-chess",
            "license": "GPL-3.0 (python-chess); GPL-3.0 (Stockfish, if used as gold source)",
            "fen": board.fen(),
            "game_idx": game_idx,
            "ply": ply,
            "gold_move_uci": position.gold_move.uci(),
            "gold_move_san": board.san(position.gold_move),
            "gold_label_method": position.gold_method,
            "n_legal_moves": len(legal_moves),
        },
    )


def generate_random_games(
    *,
    n_games: int,
    max_plies_per_game: int,
    out_dir: Path,
    modality_available: tuple[str, ...] = ("multimodal", "text"),
    seed: int = 0,
    stockfish_path: str | None = None,
    stockfish_depth: int = 10,
) -> list[CuaTask]:
    """Plays `n_games` real random-but-legal self-play games (python-chess's
    own legal-move generator picks a uniform-random legal move each ply --
    real legal chess, not fabricated positions), sampling one position per
    ply up to `max_plies_per_game`, and converts each into a CuaTask with a
    real Stockfish gold move (if `stockfish_path` resolves to a working
    engine) or the documented weaker fallback otherwise."""
    import chess
    import chess.engine

    engine = None
    if stockfish_path:
        try:
            engine = chess.engine.SimpleEngine.popen_uci(stockfish_path)
        except OSError:
            # Narrowed from a bare `except Exception`: OSError is what a
            # missing/non-executable binary at `stockfish_path` actually
            # raises (spawn failure) -- genuinely unavailable, so fall back
            # and say so via gold_method. A `chess.engine.EngineError` (the
            # binary launched but isn't behaving like a valid UCI engine) is
            # deliberately NOT caught here: silently downgrading every gold
            # move in the run to the much weaker first-legal-move fallback,
            # while looking like Stockfish was used, must fail loud instead.
            engine = None  # genuinely unavailable -- fall back, and say so via gold_method

    rng = random.Random(seed)
    tasks: list[CuaTask] = []
    try:
        for g in range(n_games):
            board = chess.Board()
            for ply in range(max_plies_per_game):
                if board.is_game_over():
                    break
                position = gold_move_for(board, engine=engine, depth=stockfish_depth)
                task = convert_position(position=position, game_idx=g, ply=ply, out_dir=out_dir,
                                         modality_available=modality_available)
                if task is not None:
                    tasks.append(task)
                move = rng.choice(list(board.legal_moves))
                board.push(move)
    finally:
        if engine is not None:
            engine.quit()
    return tasks
