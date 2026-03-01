import { redirect } from "next/navigation";

// Root → redirect to overview
export default function Home() {
  redirect("/overview");
}
