import contextlib
import io
import unittest
from unittest.mock import patch
import upgrade_debian13 as upgrade


class UpgradeTests(unittest.TestCase):
    def test_both_physical_profiles_fail_without_process_or_device_access(self):
        for arguments in ([], ['--profile', 'vitrallis-default']):
            with patch.object(upgrade.subprocess, 'run') as run, contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(upgrade.main(arguments), 2)
                run.assert_not_called()

    def test_plan_defaults_to_stock_and_never_starts_a_process(self):
        output = io.StringIO()
        with patch.object(upgrade.subprocess, 'run') as run, contextlib.redirect_stdout(output):
            self.assertEqual(upgrade.main(['--plan']), 0)
            run.assert_not_called()
        import json
        self.assertEqual(json.loads(output.getvalue())['profile'], 'stock')

    def test_simulation_passes_explicit_choice_and_propagates_failure(self):
        for profile in ('stock', 'vitrallis-default'):
            command = ['/mock/flasher-cli', 'simulate', '--profile', profile]
            with patch.object(upgrade, 'simulation_command', return_value=command) as select, patch.object(upgrade.subprocess, 'run') as run, contextlib.redirect_stdout(io.StringIO()):
                run.return_value.returncode = 1
                self.assertEqual(upgrade.main(['--simulate', '--profile', profile]), 1)
                select.assert_called_once_with(profile)
                run.assert_called_once_with(command, check=False, timeout=600)

    def test_unknown_profile_is_rejected_before_execution(self):
        with patch.object(upgrade.subprocess, 'run') as run, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                upgrade.main(['--profile', 'unknown'])
            run.assert_not_called()
