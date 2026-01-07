#!/usr/bin/env python3
"""Playwright tests for Slack environment reactions feature."""

import asyncio
from playwright.async_api import async_playwright, expect


async def test_reactions():
    """Test the reactions feature in the Slack environment."""
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        page = await browser.new_page(viewport={"width": 1400, "height": 900})

        # Navigate to the Slack app
        await page.goto("http://localhost:8080")
        await page.wait_for_load_state("networkidle")
        await asyncio.sleep(0.5)

        print("=== Testing Reactions Feature ===\n")

        # Test 1: Verify react button appears on message hover
        print("Test 1: Verify react button appears on hover...")
        first_message = await page.query_selector('.message')
        assert first_message, "Should have at least one message"

        await first_message.hover()
        await asyncio.sleep(0.2)

        react_btn = await first_message.query_selector('.react-btn')
        assert react_btn, "React button should exist"
        is_visible = await react_btn.is_visible()
        assert is_visible, "React button should be visible on hover"
        print("  ✓ React button appears on message hover\n")

        # Test 2: Click react button to open emoji picker
        print("Test 2: Open emoji picker...")
        await react_btn.click()
        await asyncio.sleep(0.2)

        emoji_picker = await first_message.query_selector('.emoji-picker.open')
        assert emoji_picker, "Emoji picker should be open"
        print("  ✓ Emoji picker opens on react button click\n")

        # Test 3: Verify emoji picker has emojis
        print("Test 3: Verify emoji picker contents...")
        emoji_buttons = await emoji_picker.query_selector_all('.emoji-picker-btn')
        assert len(emoji_buttons) >= 6, f"Should have at least 6 emojis, got {len(emoji_buttons)}"
        print(f"  ✓ Emoji picker has {len(emoji_buttons)} emojis\n")

        # Test 4: Add a reaction
        print("Test 4: Add a thumbs up reaction...")
        thumbs_up_btn = await emoji_picker.query_selector('[data-emoji="👍"]')
        assert thumbs_up_btn, "Thumbs up emoji button should exist"

        # Get message ID
        message_id = await first_message.get_attribute('data-message-id')

        await thumbs_up_btn.click()
        await asyncio.sleep(0.3)

        # Verify emoji picker closed
        emoji_picker_open = await first_message.query_selector('.emoji-picker.open')
        assert not emoji_picker_open, "Emoji picker should close after selection"

        # Verify reaction appears
        reaction_badge = await first_message.query_selector('.reaction-badge')
        assert reaction_badge, "Reaction badge should appear"

        reaction_emoji = await reaction_badge.query_selector('.reaction-emoji')
        emoji_text = await reaction_emoji.inner_text()
        assert emoji_text == '👍', f"Reaction should be thumbs up, got {emoji_text}"

        reaction_count = await reaction_badge.query_selector('.reaction-count')
        count_text = await reaction_count.inner_text()
        assert count_text == '1', f"Reaction count should be 1, got {count_text}"
        print("  ✓ Reaction added successfully\n")

        # Test 5: Verify reaction is highlighted as user-reacted
        print("Test 5: Verify user-reacted styling...")
        has_user_class = await reaction_badge.evaluate('el => el.classList.contains("user-reacted")')
        assert has_user_class, "Reaction badge should have user-reacted class"
        print("  ✓ Reaction badge shows user has reacted\n")

        # Test 6: Verify API state
        print("Test 6: Verify API state...")
        user_reactions = await page.evaluate('() => window.__slack_shell.getUserReactions()')
        assert len(user_reactions) == 1, f"Should have 1 user reaction, got {len(user_reactions)}"
        assert user_reactions[0]['emoji'] == '👍', "User reaction should be thumbs up"
        assert user_reactions[0]['messageId'] == message_id, "User reaction should be on correct message"

        reactions = await page.evaluate(f'() => window.__slack_shell.getReactions("{message_id}")')
        assert '👍' in reactions, "Reactions should include thumbs up"
        assert reactions['👍']['count'] == 1, "Thumbs up count should be 1"
        assert reactions['👍']['userReacted'] == True, "User should be marked as reacted"
        print("  ✓ API state is correct\n")

        # Test 7: Toggle reaction off
        print("Test 7: Toggle reaction off...")
        await reaction_badge.click()
        await asyncio.sleep(0.3)

        # Reaction should be removed
        reactions_after = await page.evaluate(f'() => window.__slack_shell.getReactions("{message_id}")')
        user_reactions_after = await page.evaluate('() => window.__slack_shell.getUserReactions()')

        assert len(user_reactions_after) == 0, "User reactions should be empty after toggle"
        print("  ✓ Reaction toggled off successfully\n")

        # Test 8: Add multiple reactions
        print("Test 8: Add multiple reactions to same message...")
        await first_message.hover()
        await asyncio.sleep(0.2)
        react_btn = await first_message.query_selector('.react-btn')
        await react_btn.click()
        await asyncio.sleep(0.2)

        # Add heart reaction
        emoji_picker = await first_message.query_selector('.emoji-picker.open')
        heart_btn = await emoji_picker.query_selector('[data-emoji="❤️"]')
        await heart_btn.click()
        await asyncio.sleep(0.3)

        # Add fire reaction via the + button
        add_reaction_btn = await first_message.query_selector('.add-reaction-btn')
        assert add_reaction_btn, "Add reaction button should appear"
        await add_reaction_btn.click()
        await asyncio.sleep(0.2)

        emoji_picker = await first_message.query_selector('.emoji-picker.open')
        fire_btn = await emoji_picker.query_selector('[data-emoji="🔥"]')
        await fire_btn.click()
        await asyncio.sleep(0.3)

        # Verify multiple reactions
        reaction_badges = await first_message.query_selector_all('.reaction-badge')
        assert len(reaction_badges) == 2, f"Should have 2 reaction badges, got {len(reaction_badges)}"

        user_reactions_multi = await page.evaluate('() => window.__slack_shell.getUserReactions()')
        assert len(user_reactions_multi) == 2, f"Should have 2 user reactions, got {len(user_reactions_multi)}"
        print("  ✓ Multiple reactions added successfully\n")

        # Test 9: React to different message
        print("Test 9: React to different message...")
        messages = await page.query_selector_all('.message')
        second_message = messages[1]
        second_message_id = await second_message.get_attribute('data-message-id')

        await second_message.hover()
        await asyncio.sleep(0.2)
        react_btn = await second_message.query_selector('.react-btn')
        await react_btn.click()
        await asyncio.sleep(0.2)

        emoji_picker = await second_message.query_selector('.emoji-picker.open')
        party_btn = await emoji_picker.query_selector('[data-emoji="🎉"]')
        await party_btn.click()
        await asyncio.sleep(0.3)

        # Verify reactions on second message
        reaction_badge = await second_message.query_selector('.reaction-badge')
        assert reaction_badge, "Second message should have reaction"

        user_reactions_final = await page.evaluate('() => window.__slack_shell.getUserReactions()')
        assert len(user_reactions_final) == 3, f"Should have 3 total user reactions, got {len(user_reactions_final)}"
        print("  ✓ Reaction added to different message\n")

        # Test 10: Clicking outside closes emoji picker
        print("Test 10: Click outside closes emoji picker...")
        await second_message.hover()
        await asyncio.sleep(0.2)
        react_btn = await second_message.query_selector('.react-btn')
        await react_btn.click()
        await asyncio.sleep(0.2)

        emoji_picker = await second_message.query_selector('.emoji-picker.open')
        assert emoji_picker, "Emoji picker should be open"

        # Click on body to close
        await page.click('body', position={'x': 10, 'y': 10})
        await asyncio.sleep(0.2)

        open_pickers = await page.query_selector_all('.emoji-picker.open')
        assert len(open_pickers) == 0, "All emoji pickers should be closed"
        print("  ✓ Clicking outside closes emoji picker\n")

        await browser.close()
        print("=== All Reaction Tests Passed! ===")


if __name__ == "__main__":
    asyncio.run(test_reactions())
