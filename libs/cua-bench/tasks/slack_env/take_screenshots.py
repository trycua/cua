#!/usr/bin/env python3
"""Take screenshots of all Slack environment views."""

import asyncio

from playwright.async_api import async_playwright


async def take_screenshots():
    async with async_playwright() as p:
        browser = await p.chromium.launch(headless=True)
        page = await browser.new_page(viewport={"width": 1400, "height": 900})

        # Navigate to the Slack app
        await page.goto("http://localhost:8080")
        await page.wait_for_load_state("networkidle")
        await asyncio.sleep(0.5)

        # Add some reactions to messages for the screenshots
        messages = await page.query_selector_all(".message")
        if len(messages) >= 3:
            # Add reactions to first message
            await messages[0].hover()
            await asyncio.sleep(0.2)
            react_btn = await messages[0].query_selector(".react-btn")
            await react_btn.click()
            await asyncio.sleep(0.2)
            emoji_picker = await messages[0].query_selector(".emoji-picker.open")
            thumbs_up = await emoji_picker.query_selector('[data-emoji="👍"]')
            await thumbs_up.click()
            await asyncio.sleep(0.2)

            # Add another reaction to first message
            add_btn = await messages[0].query_selector(".add-reaction-btn")
            await add_btn.click()
            await asyncio.sleep(0.2)
            emoji_picker = await messages[0].query_selector(".emoji-picker.open")
            heart = await emoji_picker.query_selector('[data-emoji="❤️"]')
            await heart.click()
            await asyncio.sleep(0.2)

            # Add reaction to second message
            await messages[1].hover()
            await asyncio.sleep(0.2)
            react_btn = await messages[1].query_selector(".react-btn")
            await react_btn.click()
            await asyncio.sleep(0.2)
            emoji_picker = await messages[1].query_selector(".emoji-picker.open")
            fire = await emoji_picker.query_selector('[data-emoji="🔥"]')
            await fire.click()
            await asyncio.sleep(0.2)

            # Add reaction to third message
            await messages[2].hover()
            await asyncio.sleep(0.2)
            react_btn = await messages[2].query_selector(".react-btn")
            await react_btn.click()
            await asyncio.sleep(0.2)
            emoji_picker = await messages[2].query_selector(".emoji-picker.open")
            eyes = await emoji_picker.query_selector('[data-emoji="👀"]')
            await eyes.click()
            await asyncio.sleep(0.2)

        # Screenshot 1: Home view with reactions
        await page.screenshot(path="screenshots/01_home_view.png")
        print("✓ Captured Home view (with reactions)")

        # Screenshot 2: DMs view
        await page.click('[data-nav="dms"]')
        await asyncio.sleep(0.3)
        await page.screenshot(path="screenshots/02_dms_view.png")
        print("✓ Captured DMs view")

        # Screenshot 3: Activity view
        await page.click('[data-nav="activity"]')
        await asyncio.sleep(0.3)
        await page.screenshot(path="screenshots/03_activity_view.png")
        print("✓ Captured Activity view")

        # Screenshot 4: Files view
        await page.click('[data-nav="files"]')
        await asyncio.sleep(0.3)
        await page.screenshot(path="screenshots/04_files_view.png")
        print("✓ Captured Files view")

        # Screenshot 5: Later view
        await page.click('[data-nav="later"]')
        await asyncio.sleep(0.3)
        await page.screenshot(path="screenshots/05_later_view.png")
        print("✓ Captured Later view")

        # Screenshot 6: More view
        await page.click('[data-nav="more"]')
        await asyncio.sleep(0.3)
        await page.screenshot(path="screenshots/06_more_view.png")
        print("✓ Captured More view")

        # Screenshot 7: Home view with emoji picker open
        await page.click('[data-nav="home"]')
        await asyncio.sleep(0.3)
        # Hover on a message to show react button
        messages = await page.query_selector_all(".message")
        if messages:
            await messages[0].hover()
            await asyncio.sleep(0.2)
            react_btn = await messages[0].query_selector(".react-btn")
            await react_btn.click()
            await asyncio.sleep(0.3)
            await page.screenshot(path="screenshots/07_emoji_picker.png")
            print("✓ Captured Emoji picker view")
            # Close picker
            await page.click("body", position={"x": 10, "y": 10})
            await asyncio.sleep(0.2)

        # Screenshot 8: Thread view
        messages = await page.query_selector_all(".message")
        if messages:
            reply_btn = await messages[0].query_selector(".reply-btn")
            await messages[0].hover()
            await asyncio.sleep(0.2)
            await reply_btn.click()
            await asyncio.sleep(0.3)
            await page.screenshot(path="screenshots/08_thread_view.png")
            print("✓ Captured Thread view")

        await browser.close()
        print("\nAll screenshots saved to screenshots/ directory")


if __name__ == "__main__":
    asyncio.run(take_screenshots())
