import React from "react";
import { Link } from "wouter";
import { useGetXclStats, useGetXclTransfers } from "@workspace/api-client-react";
import { formatNumber, formatCurrency, timeAgo, formatAddress } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { Link as LinkIcon, Activity, Globe, Clock, ArrowRight } from "lucide-react";

export default function Xcl() {
  const { data: stats, isLoading: statsLoading } = useGetXclStats();
  const { data: transfers, isLoading: transfersLoading } = useGetXclTransfers({ limit: 20 });

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
          <LinkIcon className="text-primary w-6 h-6" /> Cross-Chain (XCL)
        </h2>
        <p className="text-muted-foreground">Native trustless bridging and cross-chain message passing.</p>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Activity className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Total Transfers</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatNumber(stats?.totalTransfers || 0, 0)}
              </h3>
            </div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Globe className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">24h Volume</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatCurrency(stats?.volume24h || 0)}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><LinkIcon className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Connected Chains</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-12" /> : stats?.supportedChains || 0}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Clock className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Avg Finality</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : `${formatNumber(stats?.avgFinalizationTime || 0, 1)}s`}
              </h3>
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardContent className="p-0">
          {transfersLoading ? (
            <div className="p-4 space-y-2">
              {Array.from({ length: 10 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Tx Hash</TableHead>
                  <TableHead>Age</TableHead>
                  <TableHead>Path</TableHead>
                  <TableHead>Asset</TableHead>
                  <TableHead className="text-right">Amount</TableHead>
                  <TableHead className="text-center">Status</TableHead>
                  <TableHead>Proof</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {transfers?.map((tx) => (
                  <TableRow key={tx.id}>
                    <TableCell className="font-mono text-xs">
                      <Link href={`/txs/${tx.txHash}`} className="text-primary hover:underline">
                        {formatAddress(tx.txHash, 6)}
                      </Link>
                    </TableCell>
                    <TableCell className="text-xs text-muted-foreground">{timeAgo(tx.timestamp)}</TableCell>
                    <TableCell>
                      <div className="flex items-center gap-2 text-xs font-semibold">
                        <Badge variant="outline" className="bg-muted text-[10px]">{tx.sourceChain}</Badge>
                        <ArrowRight className="w-3 h-3 text-muted-foreground" />
                        <Badge variant="outline" className="bg-muted text-[10px]">{tx.destChain}</Badge>
                      </div>
                    </TableCell>
                    <TableCell className="font-bold">{tx.asset}</TableCell>
                    <TableCell className="text-right font-mono text-sm">{formatNumber(tx.amount, 4)}</TableCell>
                    <TableCell className="text-center">
                      <Badge variant={
                        tx.status === 'finalized' ? 'default' : 
                        tx.status === 'failed' ? 'destructive' : 
                        'secondary'
                      } className="text-[10px] uppercase">
                        {tx.status}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-xs text-muted-foreground uppercase">{tx.proofType || 'SPV'}</TableCell>
                  </TableRow>
                ))}
                {transfers?.length === 0 && (
                  <TableRow>
                    <TableCell colSpan={7} className="text-center py-8 text-muted-foreground">
                      No recent transfers.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
