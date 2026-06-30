import React from "react";
import { Link } from "wouter";
import { useGetMempoolTxs, useGetMempoolStats } from "@workspace/api-client-react";
import { formatNumber, formatAddress, timeAgo } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Badge } from "@/components/ui/badge";
import { Layers, Clock, Activity, BarChart } from "lucide-react";

export default function Mempool() {
  const { data: stats, isLoading: statsLoading } = useGetMempoolStats();
  const { data: txs, isLoading: txsLoading } = useGetMempoolTxs({ limit: 50 });

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-2xl font-bold tracking-tight flex items-center gap-2">
          <Layers className="text-primary w-6 h-6" /> Mempool
        </h2>
        <p className="text-muted-foreground">Live pending transactions waiting to be mined.</p>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Activity className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Pending / Queued</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : `${formatNumber(stats?.pendingCount || 0, 0)} / ${formatNumber(stats?.queuedCount || 0, 0)}`}
              </h3>
            </div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><BarChart className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Avg Gas Price</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : `${stats?.avgGasPrice || 0} gwei`}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><BarChart className="w-5 h-5 opacity-50" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Min Gas Price</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : `${stats?.minGasPrice || 0} gwei`}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Clock className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Oldest Tx</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-16" /> : `${stats?.oldestTxAge || 0}s`}
              </h3>
            </div>
          </CardContent>
        </Card>
      </div>

      <Card>
        <CardContent className="p-0">
          {txsLoading ? (
            <div className="p-4 space-y-2">
              {Array.from({ length: 15 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Tx Hash</TableHead>
                  <TableHead>Time in Pool</TableHead>
                  <TableHead>From / To</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead className="text-right">Gas Price</TableHead>
                  <TableHead className="text-right">Value</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {txs?.map((tx) => (
                  <TableRow key={tx.hash} className="opacity-80 hover:opacity-100 transition-opacity">
                    <TableCell className="font-mono text-xs">
                      {formatAddress(tx.hash, 8)}
                    </TableCell>
                    <TableCell className="text-xs text-muted-foreground">
                      <span className="flex items-center gap-1"><Clock className="w-3 h-3" /> {timeAgo(tx.addedAt)}</span>
                    </TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground">
                      <div className="flex flex-col gap-1">
                        <span className="flex gap-2">
                          <span className="w-4 text-center">F</span>
                          <Link href={`/address/${tx.from}`} className="hover:text-primary transition-colors">{formatAddress(tx.from, 6)}</Link>
                        </span>
                        {tx.to && (
                          <span className="flex gap-2">
                            <span className="w-4 text-center">T</span>
                            <Link href={`/address/${tx.to}`} className="hover:text-primary transition-colors">{formatAddress(tx.to, 6)}</Link>
                          </span>
                        )}
                      </div>
                    </TableCell>
                    <TableCell>
                      <Badge variant="outline" className="text-[10px] uppercase border-dashed">
                        {tx.type}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-right font-mono text-xs">
                      {tx.gasPrice} gwei
                    </TableCell>
                    <TableCell className="text-right font-mono text-xs">
                      {formatNumber(tx.value, 4)} ZBX
                    </TableCell>
                  </TableRow>
                ))}
                {(!txs || txs.length === 0) && (
                  <TableRow>
                    <TableCell colSpan={6} className="text-center py-12 text-muted-foreground">
                      Mempool is empty.
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
