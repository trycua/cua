from cua_agent.loops.qwen_xml import (
    convert_qwen_tool_args_to_computer_action,
    parse_tool_call_from_text,
    parse_tool_calls_from_text,
)


def test_xml_computer_click_with_type_parameter():
    tool_call = parse_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=type>left_click</parameter>
        <parameter=coordinate>[10, 20]</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"action": "left_click", "coordinate": [10, 20]},
    }
    assert convert_qwen_tool_args_to_computer_action(tool_call["arguments"]) == {
        "action": "left_click",
        "x": 10,
        "y": 20,
    }


def test_xml_computer_click_with_action_parameter():
    tool_call = parse_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=action>left_click</parameter>
        <parameter=coordinate>[30, 40]</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"action": "left_click", "coordinate": [30, 40]},
    }


def test_xml_computer_click_alias_with_xy_and_button():
    tool_call = parse_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=x>884</parameter>
        <parameter=y>22</parameter>
        <parameter=type>click</parameter>
        <parameter=button>left</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"x": 884, "y": 22, "action": "click", "button": "left"},
    }
    assert convert_qwen_tool_args_to_computer_action(tool_call["arguments"]) == {
        "action": "left_click",
        "x": 884,
        "y": 22,
    }


def test_xml_type_action_with_text_payload():
    tool_call = parse_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=type>type</parameter>
        <parameter=text>https://example.com</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"action": "type", "text": "https://example.com"},
    }
    assert convert_qwen_tool_args_to_computer_action(tool_call["arguments"]) == {
        "action": "type",
        "text": "https://example.com",
    }


def test_xml_text_action_without_text_payload_has_no_executable_action():
    tool_call = parse_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=type>text</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert tool_call == {"name": "computer", "arguments": {"action": "text"}}
    assert convert_qwen_tool_args_to_computer_action(tool_call["arguments"]) is None


def test_xml_key_action_with_keys_list():
    tool_call = parse_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=action>key</parameter>
        <parameter=keys>["ctrl", "a"]</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert convert_qwen_tool_args_to_computer_action(tool_call["arguments"]) == {
        "action": "keypress",
        "keys": ["ctrl", "a"],
    }


def test_direct_action_function_is_parsed_as_computer_action():
    tool_call = parse_tool_call_from_text(
        """
        <tool_call>
        <function=left_click>
        <parameter=coordinate>[50, 60]</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"coordinate": [50, 60], "action": "left_click"},
    }


def test_xml_action_flag_parameter_is_parsed_when_payload_exists():
    tool_call = parse_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=left_click>
        <parameter=coordinate>[70, 80]</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"coordinate": [70, 80], "action": "left_click"},
    }
    assert convert_qwen_tool_args_to_computer_action(tool_call["arguments"]) == {
        "action": "left_click",
        "x": 70,
        "y": 80,
    }


def test_multiple_xml_tool_calls_are_parsed_in_order():
    tool_calls = parse_tool_calls_from_text(
        """
        <tool_call>
        <function=computer><parameter=type>wait</parameter></function>
        </tool_call>
        <tool_call>
        <function=computer>
        <parameter=type>type</parameter>
        <parameter=text>done</parameter>
        </function>
        </tool_call>
        """,
        tool_call_parser="qwen_xml",
    )

    assert [tool_call["arguments"]["action"] for tool_call in tool_calls] == ["wait", "type"]


def test_malformed_xml_does_not_crash_or_parse():
    assert (
        parse_tool_calls_from_text(
            "<tool_call><function=computer><parameter=type>left_click",
            tool_call_parser="qwen_xml",
        )
        == []
    )
