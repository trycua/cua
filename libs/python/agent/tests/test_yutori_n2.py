from cua_agent.loops.yutori_n2 import (
    convert_yutori_n2_tool_call_to_completion_tool_calls,
    parse_yutori_n2_tool_call_from_text,
    parse_yutori_n2_tool_calls_from_text,
)


def test_yutori_n2_computer_batch_expands_gui_actions_in_order():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer_batch>
        <parameter=actions>
        [
          {"name": "left_click", "arguments": {"coordinates": [500, 500]}},
          {"name": "type", "arguments": {"text": "done"}}
        ]
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 1000, "y": 500},
        },
        {"name": "computer", "arguments": {"action": "type", "text": "done"}},
    ]


def test_yutori_n2_computer_batch_stops_at_first_unexecutable_action():
    tool_call = {
        "name": "computer_batch",
        "arguments": {
            "actions": [
                {"name": "left_click", "arguments": {"coordinate": "[1116"}},
                {"name": "type", "arguments": {"text": "done"}},
            ]
        },
    }

    assert (
        convert_yutori_n2_tool_call_to_completion_tool_calls(
            tool_call,
            dimensions=(2000, 1000),
        )
        == []
    )


def test_yutori_n2_computer_batch_preserves_actions_before_first_unexecutable_action():
    tool_call = {
        "name": "computer_batch",
        "arguments": {
            "actions": [
                {"name": "wait", "arguments": {}},
                {"name": "left_click", "arguments": {"coordinate": "[1116"}},
                {"name": "type", "arguments": {"text": "done"}},
            ]
        },
    }

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {"name": "computer", "arguments": {"action": "wait"}}
    ]


def test_yutori_n2_bash_is_a_function_call():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=bash>
        <parameter=command>ls -la /tmp</parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {"name": "bash", "arguments": {"command": "ls -la /tmp"}}
    ]


def test_yutori_n2_file_tools_are_function_calls():
    for function_name, args in [
        ("read", {"path": "/tmp/a.txt"}),
        ("write", {"path": "/tmp/a.txt", "content": "hello"}),
        ("edit", {"path": "/tmp/a.txt", "old": "hello", "new": "hi"}),
    ]:
        assert convert_yutori_n2_tool_call_to_completion_tool_calls(
            {"name": function_name, "arguments": args}
        ) == [{"name": function_name, "arguments": args}]


def test_yutori_n2_click_alias_with_xy_and_button():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=x>884</parameter>
        <parameter=y>22</parameter>
        <parameter=type>click</parameter>
        <parameter=button>left</parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 1768, "y": 22},
        }
    ]


def test_yutori_n2_action_flag_parameter_is_parsed_when_payload_exists():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=left_click>
        <parameter=coordinate>[250, 500]</parameter>
        </function>
        </tool_call>
        """
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"coordinate": [250, 500], "action": "left_click"},
    }
    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 500, "y": 500},
        }
    ]


def test_yutori_n2_malformed_action_flag_parameter_without_closing_angle_is_parsed():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=left_click
        </parameter>
        <parameter=x>
        94
        </parameter>
        <parameter=y>
        60
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"x": 94, "y": 60, "action": "left_click"},
    }
    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 188, "y": 60},
        }
    ]


def test_yutori_n2_malformed_coordinate_parameter_is_parsed():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=action>
        left_click
        </parameter>
        <parameter=coordinate": [500, 55]
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 1000, "y": 55},
        }
    ]


def test_yutori_n2_partial_coordinate_click_has_no_executable_action():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=triple_click>
        <parameter=coordinate>
        [1116
        </parameter>
        <parameter=type>
        click
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert tool_call == {
        "name": "computer",
        "arguments": {"action": "triple_click", "coordinate": "[1116"},
    }
    assert (
        convert_yutori_n2_tool_call_to_completion_tool_calls(
            tool_call,
            dimensions=(2000, 1000),
        )
        == []
    )


def test_yutori_n2_malformed_action_arguments_parameter_is_parsed():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=left_click, "arguments": {"action": "left_click", "coordinate": [31, 88]}}
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 62, "y": 88},
        }
    ]


def test_yutori_n2_action_parameter_can_carry_coordinates():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=left_click>
        [35, 22]
        </parameter>
        <parameter=type>
        left_click
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 70, "y": 22},
        }
    ]


def test_yutori_n2_malformed_json_tool_call_with_nested_arguments_is_parsed():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        {"name": "computer", "arguments": {"action": "left_click", "arguments": {"coordinate": [153, 65]}}
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 306, "y": 65},
        }
    ]


def test_yutori_n2_malformed_function_opener_is_parsed():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        {"function":computer>
        <parameter=action>
        left_click
        </parameter>
        <parameter=coordinate>
        [197, 58]
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 394, "y": 58},
        }
    ]


def test_yutori_n2_malformed_function_equals_opener_is_parsed():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        {"function=computer>
        <parameter=action>
        type
        </parameter>
        <parameter=text>
        https://example.com

        </parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {
            "name": "computer",
            "arguments": {"action": "type", "text": "https://example.com"},
        }
    ]


def test_yutori_n2_unclosed_function_with_combined_action_parameter_is_parsed():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=action>
        left_click, "coordinate": [130, 17]}
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        tool_call,
        dimensions=(2000, 1000),
    ) == [
        {
            "name": "computer",
            "arguments": {"action": "click", "button": "left", "x": 260, "y": 17},
        }
    ]


def test_yutori_n2_key_press_with_key_comb():
    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        {"name": "key_press", "arguments": {"key_comb": "ctrl+a"}}
    ) == [{"name": "computer", "arguments": {"action": "keypress", "keys": ["ctrl", "a"]}}]


def test_yutori_n2_key_press_with_space_separated_sequence():
    assert convert_yutori_n2_tool_call_to_completion_tool_calls(
        {"name": "key_press", "arguments": {"key_comb": "down down down enter"}}
    ) == [
        {"name": "computer", "arguments": {"action": "keypress", "keys": ["down"]}},
        {"name": "computer", "arguments": {"action": "keypress", "keys": ["down"]}},
        {"name": "computer", "arguments": {"action": "keypress", "keys": ["down"]}},
        {"name": "computer", "arguments": {"action": "keypress", "keys": ["enter"]}},
    ]


def test_yutori_n2_button_parameter_can_carry_key_action():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=button>
        key
        </parameter>
        <parameter=keys>
        ["enter"]
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {"name": "computer", "arguments": {"action": "keypress", "keys": ["enter"]}}
    ]


def test_yutori_n2_key_parameter_can_carry_implicit_keypress():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=key>
        ["ctrl", "a"]
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {"name": "computer", "arguments": {"action": "keypress", "keys": ["ctrl", "a"]}}
    ]


def test_yutori_n2_type_parameter_can_carry_text_payload():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=type>
        https://example.com
        </parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {
            "name": "computer",
            "arguments": {"action": "type", "text": "https://example.com"},
        }
    ]


def test_yutori_n2_text_parameter_preserves_inline_whitespace():
    tool_call = parse_yutori_n2_tool_call_from_text(
        "<tool_call><function=computer><parameter=type>type</parameter>"
        "<parameter=text>  spaced text  </parameter></function></tool_call>"
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {"name": "computer", "arguments": {"action": "type", "text": "  spaced text  "}}
    ]


def test_yutori_n2_text_parameter_preserves_json_string_newline():
    tool_call = parse_yutori_n2_tool_call_from_text(
        r'<tool_call><function=computer><parameter=type>type</parameter><parameter=text>"line\n"</parameter></function></tool_call>'
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {"name": "computer", "arguments": {"action": "type", "text": "line\n"}}
    ]


def test_yutori_n2_text_parameter_preserves_raw_inline_newline():
    tool_call = parse_yutori_n2_tool_call_from_text(
        "<tool_call><function=computer><parameter=type>type</parameter>"
        "<parameter=text>line\n</parameter></function></tool_call>"
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {"name": "computer", "arguments": {"action": "type", "text": "line\n"}}
    ]


def test_yutori_n2_function_tool_text_parameters_preserve_inline_whitespace():
    tool_call = parse_yutori_n2_tool_call_from_text(
        "<tool_call><function=write><parameter=path>/tmp/a.txt</parameter>"
        "<parameter=content>  hello  </parameter></function></tool_call>"
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == [
        {"name": "write", "arguments": {"path": "/tmp/a.txt", "content": "  hello  "}}
    ]


def test_yutori_n2_text_action_without_text_payload_has_no_executable_action():
    tool_call = parse_yutori_n2_tool_call_from_text(
        """
        <tool_call>
        <function=computer>
        <parameter=type>text</parameter>
        </function>
        </tool_call>
        """
    )

    assert convert_yutori_n2_tool_call_to_completion_tool_calls(tool_call) == []


def test_yutori_n2_multiple_xml_tool_calls_are_parsed_in_order():
    tool_calls = parse_yutori_n2_tool_calls_from_text(
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
        """
    )

    assert [tool_call["arguments"]["action"] for tool_call in tool_calls] == ["wait", "type"]


def test_yutori_n2_malformed_xml_does_not_crash_or_parse():
    assert (
        parse_yutori_n2_tool_calls_from_text(
            "<tool_call><function=computer><parameter=type>left_click"
        )
        == []
    )
