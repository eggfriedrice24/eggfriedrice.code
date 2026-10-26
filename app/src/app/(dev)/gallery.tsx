import { GalleryScreen } from "@/features/demo/screens/gallery-screen";

export { ErrorBoundary } from "expo-router";

// The dev gallery of every design board. Settings opens it in a dev or preview build.
export default function Gallery() {
  return <GalleryScreen />;
}
