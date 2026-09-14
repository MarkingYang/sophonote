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
    from hermes_cli.web_models import MemoryProviderConfigUpdate, MemoryProviderSelect

    async def verify():
        initial = load_config()
        initial.setdefault("memory", {})["openviking"] = {"recall_limit": 7}
        save_config(initial)
        fields = {f["key"]: f for f in (await web_server.get_memory_provider_config("openviking"))["fields"]}
        assert "endpoint" in fields
        assert fields["api_key"]["value"] == ""
        assert fields["api_key"]["is_set"] is True
        values = {"endpoint": "http://127.0.0.1:1933", "account": "default", "user": "default", "agent": "sophonote-contract"}
        response = await web_server.update_memory_provider_config("openviking", MemoryProviderConfigUpdate(values=values))
        assert response["active"] == "openviking"
        stored = load_config()
        assert stored["memory"]["provider"] == "openviking"
        for key, value in values.items():
            assert stored["memory"]["openviking"][key] == value
        assert stored["memory"]["openviking"]["recall_limit"] == 7
        provider = web_server._load_memory_provider("openviking")
        import sys
        resolved = sys.modules[type(provider).__module__]._resolve_connection_settings(stored["memory"]["openviking"])
        assert resolved["endpoint"] == values["endpoint"]
        assert resolved["agent"] == values["agent"]
        assert resolved["api_key"] == os.environ["OPENVIKING_API_KEY"]
        await web_server.set_memory_provider(MemoryProviderSelect(provider=""))
        assert load_config()["memory"]["provider"] == ""
        assert load_config()["memory"]["openviking"]["endpoint"] == values["endpoint"]
        for path in [Path(temporary) / "config.yaml", Path(temporary) / ".env"]:
            if path.exists():
                assert "contract-fixture-not-a-real-key" not in path.read_text()
        print("PASS: native schema, enable, preserve recall settings, resolve injected credential, disable, no persisted secret")

    asyncio.run(verify())
