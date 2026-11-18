import unittest

import autojoin_sidecar


class FakeMatch:
    def __init__(self):
        self.removed = False

    def remove(self):
        self.removed = True


class AvahiBrowserTests(unittest.TestCase):
    def test_speaker_resolver_failure_releases_signal_and_object(self):
        browser = autojoin_sidecar.AvahiBrowser()
        key = (1, "speaker", "_airplay._tcp")
        match = FakeMatch()
        freed = []
        browser.generation = 3
        browser.resolve_attempt[key] = 1
        browser.resolvers[key] = "/resolver/speaker"
        browser.resolver_signals[key] = [match]
        browser.unresolved.add(key)
        browser._free = lambda path, kind: freed.append((path, kind))

        browser._on_resolver_failure(3, key, 1, "failed")

        self.assertTrue(match.removed)
        self.assertEqual(browser.resolver_signals, {})
        self.assertEqual(browser.resolvers[key], "")
        self.assertNotIn(key, browser.unresolved)
        self.assertEqual(freed, [("/resolver/speaker", "ServiceResolver")])

    def test_wled_service_removal_releases_resolver_rules_under_churn(self):
        browser = autojoin_sidecar.AvahiBrowser()
        freed = []
        browser._free = lambda path, kind: freed.append((path, kind))

        for index in range(600):
            key = (1, f"controller-{index}", "_wled._tcp")
            match = FakeMatch()
            path = f"/resolver/{index}"
            browser.wled_resolvers[key] = path
            browser.wled_resolver_signals[key] = [match]
            browser.wled_matches.append(match)

            browser._on_wled_remove(
                browser.wled_generation,
                key[0],
                0,
                key[1],
                key[2],
                "local",
                0,
            )

            self.assertTrue(match.removed)
            self.assertNotIn(key, browser.wled_resolvers)
            self.assertNotIn(key, browser.wled_resolver_signals)
            self.assertEqual(browser.wled_matches, [])

        self.assertEqual(
            freed,
            [(f"/resolver/{index}", "ServiceResolver") for index in range(600)],
        )

    def test_wled_resolver_success_releases_signal_and_object(self):
        browser = autojoin_sidecar.AvahiBrowser()
        key = (1, "controller", "_wled._tcp")
        match = FakeMatch()
        freed = []
        browser.wled_resolvers[key] = "/resolver/controller"
        browser.wled_resolver_signals[key] = [match]
        browser.wled_matches.append(match)
        browser._free = lambda path, kind: freed.append((path, kind))

        browser._on_wled_found(
            browser.wled_generation,
            key,
            1,
            0,
            "Controller",
            "_wled._tcp",
            "local",
            "controller.local",
            0,
            "192.0.2.10",
            11988,
            [],
            0,
        )

        self.assertTrue(match.removed)
        self.assertEqual(browser.wled_matches, [])
        self.assertEqual(browser.wled_resolvers[key], "")
        self.assertEqual(freed, [("/resolver/controller", "ServiceResolver")])
        self.assertTrue(autojoin_sidecar._wled_rows["192.0.2.10"]["online"])
        autojoin_sidecar._wled_rows.pop("192.0.2.10", None)


class DiscoveryHelperTests(unittest.TestCase):
    def test_alias_target_matches_device_id(self):
        config = object()
        found = autojoin_sidecar.ScanIndex(
            devices=[],
            self_id="",
            self_deviceid="",
            by_id={"speaker-id": config},
            props_by_id={"speaker-id": {"deviceid": "AA:BB:CC:DD:EE:FF"}},
            names={"speaker-id": "Speaker"},
            meta={},
        )

        autojoin_sidecar.alias_target(found, "aabbccddeeff", "")

        self.assertIs(found.by_id["aabbccddeeff"], config)
        self.assertEqual(found.names["aabbccddeeff"], "Speaker")

    def test_raop_refresh_preserves_airplay_name_and_properties(self):
        record = autojoin_sidecar._new_record(
            "Speaker", "192.0.2.1", {"model": "Model", "deviceid": "id"}, False
        )

        autojoin_sidecar._refresh_record(
            record, "AA:BB@Speaker", "192.0.2.2", {"raop_only": "value"}, True
        )

        self.assertEqual(record["name"], "Speaker")
        self.assertEqual(record["address"], "192.0.2.2")
        self.assertEqual(record["model"], "Model")
        self.assertEqual(
            record["props"],
            {"raop_only": "value", "model": "Model", "deviceid": "id"},
        )


if __name__ == "__main__":
    unittest.main()