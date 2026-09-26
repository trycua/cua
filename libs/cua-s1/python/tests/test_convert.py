from __future__ import annotations

import pytest

from cua_s1.checkpoint import load_checkpoint_files
from cua_s1.convert import convert_archive, convert_file, main


def _archive(torch):
    return {
        "config": {"encoder": "tiny", "width": 3, "rank": 2, "context_tokens": 8},
        "state_dict": {"embedding.weight": torch.ones(2, 3), "head.bias": torch.zeros(3)},
        "history": [{"epoch": 1, "val_top1": 0.5}],
    }


def test_converted_archive_loads_through_the_safe_reader(tmp_path):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")
    archive = _archive(torch)

    weights_path, config_path = convert_archive(archive, tmp_path / "checkpoint", "model.pt")
    state, config, metadata = load_checkpoint_files(tmp_path / "checkpoint")

    assert weights_path.suffix == ".safetensors"
    assert config_path.suffix == ".json"
    assert config == archive["config"]
    assert metadata == {"history": archive["history"], "source": "model.pt"}
    assert set(state) == set(archive["state_dict"])
    for name, tensor in archive["state_dict"].items():
        assert torch.equal(state[name], tensor)


def test_convert_file_reads_a_pickle_archive_and_produces_a_loadable_pair(tmp_path):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")
    source = tmp_path / "model.pt"
    torch.save(_archive(torch), source)

    convert_file(source, tmp_path / "checkpoint")
    state, config, metadata = load_checkpoint_files(tmp_path / "checkpoint")

    assert config["encoder"] == "tiny"
    assert metadata["source"] == "model.pt"
    assert torch.equal(state["embedding.weight"], torch.ones(2, 3))


def test_convert_rejects_an_archive_without_config_or_state(tmp_path):
    torch = pytest.importorskip("torch")
    with pytest.raises(ValueError, match="missing required fields"):
        convert_archive({"state_dict": {"w": torch.ones(1)}}, tmp_path / "checkpoint")


def test_convert_names_the_field_that_cannot_be_serialized(tmp_path):
    torch = pytest.importorskip("torch")
    archive = _archive(torch)
    archive["extra"] = {1, 2}
    with pytest.raises(ValueError, match="'extra' is not JSON serializable"):
        convert_archive(archive, tmp_path / "checkpoint")


def test_convert_reports_a_missing_source_file(tmp_path):
    with pytest.raises(FileNotFoundError):
        convert_file(tmp_path / "absent.pt", tmp_path / "checkpoint")


def test_command_line_conversion_writes_both_files(tmp_path, capsys):
    torch = pytest.importorskip("torch")
    pytest.importorskip("safetensors")
    source = tmp_path / "model.pt"
    torch.save(_archive(torch), source)

    assert main([str(source), str(tmp_path / "checkpoint")]) == 0

    assert (tmp_path / "checkpoint" / "model.safetensors").is_file()
    assert (tmp_path / "checkpoint" / "config.json").is_file()
    assert "model.safetensors" in capsys.readouterr().out
