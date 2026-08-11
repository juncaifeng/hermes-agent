"""Behavior tests for the skills write-approval REST endpoints.

Thin wrappers over tools/write_approval.py (the pending store the CLI
`/skills pending|approve|reject|diff` handlers drive) plus the learning-feed
usage listing. The suite exercises the real stage → list → approve/reject →
on-disk outcome chain against an isolated HERMES_HOME.
"""
import json

import pytest


def _stage(skill_name="review-me", *, action="create", origin="background_review"):
    """Stage a skill write the way the gated skill_manage path does."""
    from tools import write_approval as wa

    content = (
        f"---\nname: {skill_name}\ndescription: A staged skill.\n---\n\n# {skill_name}\n\nBody.\n"
    )
    payload = {"action": action, "name": skill_name}
    if action in {"create", "edit"}:
        payload["content"] = content
    gist = wa.skill_gist(action, skill_name, content=payload.get("content", ""))
    return wa.stage_write(wa.SKILLS, payload, summary=gist, origin=origin)


@pytest.fixture
def client(tmp_path, monkeypatch, _isolate_hermes_home):
    try:
        from starlette.testclient import TestClient
    except ImportError:
        pytest.skip("fastapi/starlette not installed")

    import hermes_state
    from hermes_constants import get_hermes_home
    from hermes_cli.web_server import app, _SESSION_HEADER_NAME, _SESSION_TOKEN

    (get_hermes_home() / "skills").mkdir(parents=True, exist_ok=True)
    (get_hermes_home() / "config.yaml").write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(hermes_state, "DEFAULT_DB_PATH", get_hermes_home() / "state.db")
    c = TestClient(app)
    c.headers[_SESSION_HEADER_NAME] = _SESSION_TOKEN
    return c


class TestPendingList:
    def test_staged_write_appears_in_pending_list(self, client):
        rec = _stage()

        resp = client.get("/api/skills/pending")

        assert resp.status_code == 200
        rows = resp.json()
        assert [r["id"] for r in rows] == [rec["id"]]
        row = rows[0]
        assert row["action"] == "create"
        assert row["name"] == "review-me"
        assert row["origin"] == "background_review"
        assert row["gist"]
        assert row["created_at"] > 0
        # The payload itself (full content) is not part of the list shape.
        assert "payload" not in row

    def test_empty_queue_lists_nothing(self, client):
        resp = client.get("/api/skills/pending")

        assert resp.status_code == 200
        assert resp.json() == []


class TestApprove:
    def test_approve_replays_the_write_and_drops_the_record(self, client):
        from hermes_constants import get_hermes_home

        rec = _stage()

        resp = client.post(f"/api/skills/pending/{rec['id']}/approve")

        assert resp.status_code == 200
        assert resp.json()["success"] is True
        # The skill landed on disk…
        skill_md = get_hermes_home() / "skills" / "review-me" / "SKILL.md"
        assert skill_md.exists()
        assert "A staged skill." in skill_md.read_text(encoding="utf-8")
        # …and the queue no longer holds the record.
        assert client.get("/api/skills/pending").json() == []

    def test_approve_unknown_id_404(self, client):
        resp = client.post("/api/skills/pending/nope0000/approve")

        assert resp.status_code == 404

    def test_failed_replay_keeps_the_record(self, client):
        """A write that fails validation on replay must stay in the queue —
        the user hasn't actually rejected it."""
        rec = _stage()
        # Sabotage the staged payload so the replay fails validation.
        from tools import write_approval as wa

        broken = wa.get_pending(wa.SKILLS, rec["id"])
        broken["payload"]["name"] = "INVALID NAME WITH SPACES"
        path = get_pending_path(rec["id"])
        path.write_text(json.dumps(broken), encoding="utf-8")

        resp = client.post(f"/api/skills/pending/{rec['id']}/approve")

        assert resp.status_code == 400
        assert wa.get_pending(wa.SKILLS, rec["id"]) is not None


class TestReject:
    def test_reject_discards_without_writing(self, client):
        from hermes_constants import get_hermes_home

        rec = _stage()

        resp = client.post(f"/api/skills/pending/{rec['id']}/reject")

        assert resp.status_code == 200
        assert resp.json() == {"ok": True, "id": rec["id"]}
        assert not (get_hermes_home() / "skills" / "review-me").exists()
        assert client.get("/api/skills/pending").json() == []

    def test_reject_unknown_id_404(self, client):
        resp = client.post("/api/skills/pending/nope0000/reject")

        assert resp.status_code == 404


class TestDiff:
    def test_diff_for_create_returns_full_content(self, client):
        rec = _stage()

        resp = client.get(f"/api/skills/pending/{rec['id']}/diff")

        assert resp.status_code == 200
        body = resp.json()
        assert body["id"] == rec["id"]
        assert body["action"] == "create"
        assert body["name"] == "review-me"
        assert body["gist"]
        assert body["file_path"] == "SKILL.md"
        assert "A staged skill." in body["diff"]

    def test_diff_unknown_id_404(self, client):
        resp = client.get("/api/skills/pending/nope0000/diff")

        assert resp.status_code == 404


class TestUsage:
    def test_usage_rows_sorted_by_recent_activity(self, client):
        from tools.skill_usage import load_usage, save_usage

        usage = load_usage()
        usage["old-skill"] = {
            "use_count": 1,
            "patch_count": 0,
            "last_used_at": "2026-01-01T00:00:00+00:00",
            "state": "stale",
        }
        usage["fresh-skill"] = {
            "use_count": 2,
            "patch_count": 3,
            "last_used_at": "2026-08-01T00:00:00+00:00",
            "last_patched_at": "2026-08-02T00:00:00+00:00",
            "state": "active",
        }
        save_usage(usage)

        resp = client.get("/api/skills/usage")

        assert resp.status_code == 200
        rows = resp.json()
        assert [r["name"] for r in rows] == ["fresh-skill", "old-skill"]
        fresh = rows[0]
        assert fresh["patch_count"] == 3
        assert fresh["state"] == "active"
        assert fresh["last_activity_at"] == "2026-08-02T00:00:00+00:00"


def get_pending_path(pending_id):
    from hermes_constants import get_hermes_home

    return get_hermes_home() / "pending" / "skills" / f"{pending_id}.json"
