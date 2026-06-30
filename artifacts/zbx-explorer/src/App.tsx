import { Switch, Route, Router as WouterRouter } from "wouter";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Toaster } from "@/components/ui/toaster";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ThemeProvider } from "next-themes";
import NotFound from "@/pages/not-found";

import Layout from "@/components/layout/Layout";
import Home from "@/pages/Home";
import Blocks from "@/pages/blocks";
import BlockDetail from "@/pages/blocks/detail";
import Txs from "@/pages/txs";
import TxDetail from "@/pages/txs/detail";
import Validators from "@/pages/validators";
import ValidatorDetail from "@/pages/validators/detail";
import Defi from "@/pages/defi";
import AI from "@/pages/ai";
import Xcl from "@/pages/xcl";
import Oracle from "@/pages/oracle";
import Governance from "@/pages/governance";
import ProposalDetail from "@/pages/governance/detail";
import NFT from "@/pages/nft";
import Mempool from "@/pages/mempool";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      staleTime: 5000,
    },
  },
});

function Router() {
  return (
    <Layout>
      <Switch>
        <Route path="/" component={Home} />
        <Route path="/blocks" component={Blocks} />
        <Route path="/blocks/:blockNumber" component={BlockDetail} />
        <Route path="/txs" component={Txs} />
        <Route path="/txs/:hash" component={TxDetail} />
        <Route path="/validators" component={Validators} />
        <Route path="/validators/:address" component={ValidatorDetail} />
        <Route path="/defi" component={Defi} />
        <Route path="/ai" component={AI} />
        <Route path="/xcl" component={Xcl} />
        <Route path="/oracle" component={Oracle} />
        <Route path="/governance" component={Governance} />
        <Route path="/governance/:id" component={ProposalDetail} />
        <Route path="/nft" component={NFT} />
        <Route path="/mempool" component={Mempool} />
        <Route component={NotFound} />
      </Switch>
    </Layout>
  );
}

function App() {
  return (
    <ThemeProvider attribute="class" defaultTheme="dark" enableSystem={false}>
      <QueryClientProvider client={queryClient}>
        <TooltipProvider>
          <WouterRouter base={import.meta.env.BASE_URL.replace(/\/$/, "")}>
            <Router />
          </WouterRouter>
          <Toaster />
        </TooltipProvider>
      </QueryClientProvider>
    </ThemeProvider>
  );
}

export default App;
