// The smoke test of the component and router harness: jest-expo, React Native Testing
// Library and the testing library of expo-router. If it fails, the harness is broken, not
// a screen. The tests of the screens and the components are beside them in src/.
import { describe, expect, it, jest } from "@jest/globals";
import { Stack, router } from "expo-router";
import { act, renderRouter, screen as routerScreen } from "expo-router/testing-library";
import { fireEvent, render, screen } from "@testing-library/react-native";
import { Pressable, Text } from "react-native";

interface AddButtonProps {
  readonly count: number;
  readonly onPress: () => void;
}

function AddButton(props: AddButtonProps) {
  return (
    <Pressable accessibilityRole="button" accessibilityLabel="Add one" onPress={props.onPress}>
      <Text>{`count ${props.count}`}</Text>
    </Pressable>
  );
}

function Conversation() {
  return <Text>conversation</Text>;
}

const routes = {
  _layout: () => <Stack />,
  index: () => <Text>root</Text>,
  "c/[machine]/[id]": Conversation,
};

describe("the component harness", () => {
  it("renders a component and presses it", async () => {
    // RNTL 14 renders and fires events asynchronously.
    const onPress = jest.fn<() => void>();
    await render(<AddButton count={1} onPress={onPress} />);
    expect(screen.getByText("count 1")).toBeOnTheScreen();
    await fireEvent.press(screen.getByRole("button", { name: "Add one" }));
    expect(onPress).toHaveBeenCalledTimes(1);
  });
});

describe("the router harness", () => {
  it("pushes a conversation and goes back to the root", async () => {
    await renderRouter(routes, { initialUrl: "/" });
    expect(routerScreen).toHavePathname("/");
    // A router call applies only inside an async act.
    await act(async () => {
      router.push("/c/m1/a");
    });
    expect(routerScreen).toHavePathname("/c/m1/a");
    expect(routerScreen.getByText("conversation")).toBeOnTheScreen();
    await act(async () => {
      router.dismissAll();
    });
    expect(routerScreen).toHavePathname("/");
  });
});
