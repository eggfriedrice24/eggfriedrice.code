import { Stack } from "expo-router";
import { StyleSheet, Text, View } from "react-native";

import { semantic } from "@/theme/palette";

const options = { title: "efr" };

// A placeholder until the pairing and conversation screens exist.
export default function Home() {
  return (
    <View style={styles.root}>
      <Stack.Screen options={options} />
      <Text style={styles.text}>To pair this phone, run efr device add on your computer.</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    padding: 24,
    backgroundColor: semantic.surface_1,
  },
  text: {
    color: semantic.text_primary,
    fontSize: 16,
    textAlign: "center",
  },
});
