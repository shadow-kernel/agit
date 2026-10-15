from agit import NotFound, Webhook


def test_webhooks_roundtrip(server, client):
    assert client.webhooks("demo") == []
    wh = client.create_webhook("demo", "http://localhost:9/hook", kinds=["ref.*", "pull.open"], secret="s3cret")
    assert isinstance(wh, Webhook) and wh.has_secret and wh.kinds == ["ref.*", "pull.open"]
    assert server.last.method == "POST" and server.last.json == {
        "url": "http://localhost:9/hook", "kinds": ["ref.*", "pull.open"], "secret": "s3cret"}
    assert [w.id for w in client.webhooks("demo")] == [wh.id]
    assert client.test_webhook("demo", wh.id) == {"queued": True}
    assert client.delete_webhook("demo", wh.id) == {"deleted": wh.id}
    assert client.webhooks("demo") == []
    try:
        client.delete_webhook("demo", wh.id)
    except NotFound as e:
        assert e.status == 404
    else:
        raise AssertionError("expected NotFound")
