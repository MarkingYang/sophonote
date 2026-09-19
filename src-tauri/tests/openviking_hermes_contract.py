"""Run with bundled Hermes Python/PYTHONPATH; never touches the user's Hermes Home."""
import asyncio
import os
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parents[2]
(ROOT / "logs").mkdir(exist_ok=True)

with tempfile.TemporaryDirectory(prefix="openviking-contract-", dir=ROOT / "logs") as temporary:
    os.environ["HERMES_HOME"] = temporary
    for key in list(os.environ):
        if key.startswith("OPENVIKING_"):
            del os.environ[key]
    # Models the Host-only child environment injection. Not a real credential.
    os.environ["OPENVIKING_API_KEY"] = "contract-fixture-not-a-real-key"
    from hermes_cli import web_server
    from hermes_cli.config import load_config, save_config
    from hermes_cli.web_models import ConfigUpdate

    async def verify():
        initial = load_config()
        initial.setdefault("memory", {})["openviking"] = {"recall_limit": 7}
        save_config(initial)
        fields = {f["key"]: f for f in (await web_server.get_memory_provider_config("openviking"))["fields"]}
        assert "endpoint" in fields
        assert fields["api_key"]["value"] == ""
        assert fields["api_key"]["is_set"] is True
        values = {"endpoint": "https://api.vikingdb.cn-beijing.volces.com/openviking", "account": "", "user": "", "agent": "hermes"}
        response = await web_server.update_config(ConfigUpdate(config={"memory": {"provider": "openviking", "openviking": values, "memory_enabled": False, "user_profile_enabled": False, "sophonote_engines": ["pi", "claude_code", "opencode"]}}))
        assert response["ok"] is True
        stored = load_config()
        assert stored["memory"]["provider"] == "openviking"
        assert stored["memory"]["sophonote_engines"] == ["pi", "claude_code", "opencode"]
        assert not stored["memory"]["memory_enabled"]
        assert not stored["memory"]["user_profile_enabled"]
        for key, value in values.items():
            assert stored["memory"]["openviking"][key] == value
        assert stored["memory"]["openviking"]["recall_limit"] == 7
        provider = web_server._load_memory_provider("openviking")
        import sys
        resolved = sys.modules[type(provider).__module__]._resolve_connection_settings(stored["memory"]["openviking"])
        assert resolved["endpoint"] == values["endpoint"]
        assert resolved["agent"] == values["agent"]
        assert resolved["api_key"] == os.environ["OPENVIKING_API_KEY"]
        module = sys.modules[type(provider).__module__]
        class CloudClient:
            _api_key = "contract-fixture-not-a-real-key"
            def health_payload(self):
                raise AssertionError("Managed cloud must not depend on anonymous health")
            def get(self, path, params):
                assert path == "/api/v1/fs/ls" and params["uri"] == "viking://~"
                return {"status": "ok", "result": []}
        cloud_client = CloudClient()
        assert module._classify_runtime_openviking_health(cloud_client, values["endpoint"])[0] == "healthy"
        cloud_client._api_key = ""
        assert module._classify_runtime_openviking_health(cloud_client, values["endpoint"])[0] != "healthy"

        await web_server.update_config(ConfigUpdate(config={"memory": {"provider": "", "memory_enabled": False, "user_profile_enabled": False}}))
        assert load_config()["memory"]["provider"] == ""
        assert load_config()["memory"]["openviking"]["endpoint"] == values["endpoint"]
        for path in [Path(temporary) / "config.yaml", Path(temporary) / ".env"]:
            if path.exists():
                assert "contract-fixture-not-a-real-key" not in path.read_text()
        print("PASS: cloud config, local memory disabled, preserve recall settings, injected credential, disable without local fallback, no persisted secret")

    asyncio.run(verify())
