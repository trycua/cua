"""
Tests for centralized tool resolution in ComputerAgent.

Tests that:
1. FARA (specialized model) auto-wraps Computer to BrowserTool with warning
2. FARA accepts explicit BrowserTool without warning
3. Claude (general model) accepts any tool without wrapping
4. Custom function tools pass through unchanged
"""

import warnings
from unittest.mock import MagicMock, Mock, patch

import pytest
from agent.computers import is_agent_computer
from agent.tools.browser_tool import BrowserTool
from agent.types import AgentConfigInfo


# Mock agent config class for testing
class MockAgentConfig:
    """Mock agent config class for testing"""

    async def predict_step(self, **kwargs):
        return {"output": [], "usage": {}}

    async def predict_click(self, **kwargs):
        return None

    def get_capabilities(self):
        return ["step"]


class TestToolResolution:
    """Tests for ComputerAgent._resolve_tools()"""

    @pytest.fixture
    def mock_computer(self):
        """Create a mock Computer object"""
        computer = Mock()
        computer.interface = Mock()
        computer.interface.interface = Mock()  # For hotkey, etc.
        computer.interface.playwright_exec = Mock()
        return computer

    @pytest.fixture
    def mock_browser_tool(self):
        """Create a mock BrowserTool"""
        tool = Mock(spec=BrowserTool)
        return tool

    def test_fara_auto_wraps_computer_to_browser_tool(self, mock_computer):
        """FARA auto-wraps Computer to BrowserTool with warning."""
        from agent.agent import ComputerAgent

        # Patch find_agent_config to return FARA config
        fara_config = AgentConfigInfo(
            agent_class=MockAgentConfig,
            models_regex=r"(?i).*fara.*",
            priority=0,
            tool_type="browser",
        )

        # Patch is_agent_computer to recognize our mock as a computer
        def mock_is_agent_computer(tool):
            return tool is mock_computer

        with patch("agent.agent.find_agent_config", return_value=fara_config):
            with patch("agent.agent.is_agent_computer", side_effect=mock_is_agent_computer):
                with pytest.warns(UserWarning, match="Auto-wrapping Computer to BrowserTool"):
                    agent = ComputerAgent(model="cua/microsoft/fara-7b", tools=[mock_computer])

        # Should have wrapped to BrowserTool
        assert len(agent.tools) == 1
        assert isinstance(agent.tools[0], BrowserTool)

    def test_fara_accepts_explicit_browser_tool_no_warning(self, mock_browser_tool):
        """FARA accepts explicit BrowserTool without warning."""
        from agent.agent import ComputerAgent

        # Patch find_agent_config to return FARA config
        fara_config = AgentConfigInfo(
            agent_class=MockAgentConfig,
            models_regex=r"(?i).*fara.*",
            priority=0,
            tool_type="browser",
        )

        with patch("agent.agent.find_agent_config", return_value=fara_config):
            # Should not raise any warnings
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                agent = ComputerAgent(model="cua/microsoft/fara-7b", tools=[mock_browser_tool])

        # Should keep the original BrowserTool
        assert len(agent.tools) == 1
        assert agent.tools[0] is mock_browser_tool

    def test_claude_accepts_any_tool_no_wrapping(self, mock_computer):
        """Claude (general model) accepts any tool without wrapping."""
        from agent.agent import ComputerAgent

        # Patch find_agent_config to return Claude config (no tool_type)
        claude_config = AgentConfigInfo(
            agent_class=MockAgentConfig,
            models_regex=r".*claude.*",
            priority=0,
            tool_type=None,  # General model, no tool type requirement
        )

        with patch("agent.agent.find_agent_config", return_value=claude_config):
            with warnings.catch_warnings():
                warnings.simplefilter("error")  # Fail if any warning
                agent = ComputerAgent(model="claude-sonnet-4", tools=[mock_computer])

        # Should keep original Computer, not wrapped
        assert len(agent.tools) == 1
        assert agent.tools[0] is mock_computer

    def test_custom_tools_pass_through(self, mock_computer):
        """Custom function tools pass through unchanged."""
        from agent.agent import ComputerAgent

        def custom_tool():
            """A custom tool function."""
            pass

        # Patch find_agent_config to return FARA config
        fara_config = AgentConfigInfo(
            agent_class=MockAgentConfig,
            models_regex=r"(?i).*fara.*",
            priority=0,
            tool_type="browser",
        )

        # Patch is_agent_computer to recognize our mock as a computer
        def mock_is_agent_computer(tool):
            return tool is mock_computer

        with patch("agent.agent.find_agent_config", return_value=fara_config):
            with patch("agent.agent.is_agent_computer", side_effect=mock_is_agent_computer):
                # Suppress the warning for computer wrapping
                with warnings.catch_warnings():
                    warnings.simplefilter("ignore")
                    agent = ComputerAgent(
                        model="cua/microsoft/fara-7b", tools=[mock_computer, custom_tool]
                    )

        # Should have wrapped computer but kept custom tool unchanged
        assert len(agent.tools) == 2
        assert isinstance(agent.tools[0], BrowserTool)
        assert agent.tools[1] is custom_tool


class TestAgentConfigInfo:
    """Tests for AgentConfigInfo with tool_type field"""

    def test_agent_config_info_with_tool_type(self):
        """AgentConfigInfo accepts tool_type parameter"""
        config = AgentConfigInfo(
            agent_class=MockAgentConfig,
            models_regex=r".*fara.*",
            priority=0,
            tool_type="browser",
        )
        assert config.tool_type == "browser"

    def test_agent_config_info_without_tool_type(self):
        """AgentConfigInfo defaults tool_type to None"""
        config = AgentConfigInfo(
            agent_class=MockAgentConfig,
            models_regex=r".*claude.*",
            priority=0,
        )
        assert config.tool_type is None

    def test_agent_config_info_matches_model(self):
        """AgentConfigInfo.matches_model works correctly"""
        config = AgentConfigInfo(
            agent_class=MockAgentConfig,
            models_regex=r"(?i).*fara-7b.*",
            priority=0,
            tool_type="browser",
        )
        assert config.matches_model("cua/microsoft/fara-7b")
        assert config.matches_model("FARA-7B")
        assert not config.matches_model("claude-sonnet-4")


class TestRegisterAgentDecorator:
    """Tests for @register_agent decorator with tool_type"""

    def test_register_agent_with_tool_type(self):
        """@register_agent accepts tool_type parameter"""
        from agent.decorators import _agent_configs, register_agent

        # Clear registry for test
        original_configs = _agent_configs.copy()
        _agent_configs.clear()

        try:

            @register_agent(models=r"test-model.*", tool_type="browser")
            class TestAgentConfig:
                async def predict_step(self, **kwargs):
                    pass

                async def predict_click(self, **kwargs):
                    pass

                def get_capabilities(self):
                    return ["step"]

            # Find the registered config
            from agent.decorators import find_agent_config

            config = find_agent_config("test-model-123")
            assert config is not None
            assert config.tool_type == "browser"

        finally:
            # Restore original registry
            _agent_configs.clear()
            _agent_configs.extend(original_configs)

    def test_register_agent_without_tool_type(self):
        """@register_agent without tool_type defaults to None"""
        from agent.decorators import _agent_configs, register_agent

        # Clear registry for test
        original_configs = _agent_configs.copy()
        _agent_configs.clear()

        try:

            @register_agent(models=r"general-model.*")
            class GeneralAgentConfig:
                async def predict_step(self, **kwargs):
                    pass

                async def predict_click(self, **kwargs):
                    pass

                def get_capabilities(self):
                    return ["step"]

            # Find the registered config
            from agent.decorators import find_agent_config

            config = find_agent_config("general-model-123")
            assert config is not None
            assert config.tool_type is None

        finally:
            # Restore original registry
            _agent_configs.clear()
            _agent_configs.extend(original_configs)
