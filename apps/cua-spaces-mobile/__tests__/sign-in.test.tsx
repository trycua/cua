// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen } from "@testing-library/react-native";

import { SignInScreen } from "../src/features/auth/SignInScreen";
import { ThemeProvider } from "../src/theme/theme";

const mockSignIn = jest.fn();
let mockError: string | null = null;
jest.mock("../src/state/app", () => ({ useApp: () => ({ signIn: mockSignIn, signingIn: false, signInError: mockError }) }));

describe("sign-in", () => {
  it("starts the cua.ai sign-in sheet from one button and says why it failed", async () => {
    mockError = "Sign-in didn't finish. It was declined in the browser.";
    await render(
      <ThemeProvider force="light">
        <SignInScreen />
      </ThemeProvider>,
    );
    await fireEvent.press(screen.getByLabelText("Sign in with your cua.ai account"));
    expect(mockSignIn).toHaveBeenCalled();
    expect(screen.getByText(/declined in the browser/)).toBeTruthy();
    expect(screen.queryByTestId("use-sample")).toBeNull();
  });
});
