import React from "react";
import { Link } from "wouter";
import { useGetNetworkOverview, useGetNetworkStats, useGetBlocks, useGetTransactions } from "@workspace/api-client-react";
import { formatNumber, timeAgo, formatAddress } from "@/lib/utils";
import { Activity, Box, ListOrdered, Clock, Server, Zap, Users, Globe, ChevronRight } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { Badge } from "@/components/ui/badge";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

export default function Home() {
  const { data: overview, isLoading: overviewLoading } = useGetNetworkOverview();
  const { data: stats, isLoading: statsLoading } = useGetNetworkStats();
  const { data: blocks, isLoading: blocksLoading } = useGetBlocks({ limit: 10 });
  const { data: txs, isLoading: txsLoading } = useGetTransactions({ limit: 10 });

  return (
    <div className="space-y-6">
      <div className="flex flex-col md:flex-row justify-between items-start md:items-center gap-4">
        <div>
          <h2 className="text-2xl font-bold tracking-tight">Network Dashboard</h2>
          <p className="text-muted-foreground">Live Zebvix network statistics and latest activity.</p>
        </div>
      </div>

      {/* Network Stats Grid */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Box className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Latest Block</p>
              <h3 className="text-xl font-bold font-mono">
                {statsLoading ? <Skeleton className="h-7 w-24" /> : formatNumber(stats?.blockHeight || 0, 0)}
              </h3>
            </div>
          </CardContent>
        </Card>
        
        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Zap className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Live TPS</p>
              <h3 className="text-xl font-bold font-mono">
                {overviewLoading ? <Skeleton className="h-7 w-16" /> : formatNumber(overview?.tps || 0, 1)}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Clock className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Finality</p>
              <h3 className="text-xl font-bold font-mono">
                {overviewLoading ? <Skeleton className="h-7 w-16" /> : `${formatNumber(overview?.finalityTime || 0, 2)}s`}
              </h3>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardContent className="p-4 flex items-center gap-4">
            <div className="p-3 bg-primary/10 text-primary rounded-lg"><Users className="w-5 h-5" /></div>
            <div>
              <p className="text-xs text-muted-foreground font-semibold uppercase tracking-wider">Validators</p>
              <h3 className="text-xl font-bold font-mono">
                {overviewLoading ? <Skeleton className="h-7 w-16" /> : overview?.activeValidators || 0}
              </h3>
            </div>
          </CardContent>
        </Card>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
        <Card>
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle className="text-base font-semibold">Recent Blocks</CardTitle>
            <Link href="/blocks" className="text-xs text-primary flex items-center hover:underline">
              View All <ChevronRight className="w-3 h-3 ml-1" />
            </Link>
          </CardHeader>
          <CardContent>
            {blocksLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
              </div>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Block</TableHead>
                    <TableHead>Age</TableHead>
                    <TableHead>Txs</TableHead>
                    <TableHead className="text-right">Gas Used</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {blocks?.slice(0, 5).map((block) => (
                    <TableRow key={block.number}>
                      <TableCell className="font-mono">
                        <Link href={`/blocks/${block.number}`} className="text-primary hover:underline">
                          {block.number}
                        </Link>
                      </TableCell>
                      <TableCell className="text-xs text-muted-foreground">{timeAgo(block.timestamp)}</TableCell>
                      <TableCell>{block.txCount}</TableCell>
                      <TableCell className="text-right font-mono text-xs">{formatNumber(block.gasUsed, 0)}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="flex flex-row items-center justify-between pb-2">
            <CardTitle className="text-base font-semibold">Recent Transactions</CardTitle>
            <Link href="/txs" className="text-xs text-primary flex items-center hover:underline">
              View All <ChevronRight className="w-3 h-3 ml-1" />
            </Link>
          </CardHeader>
          <CardContent>
            {txsLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 5 }).map((_, i) => <Skeleton key={i} className="h-12 w-full" />)}
              </div>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Hash</TableHead>
                    <TableHead>From / To</TableHead>
                    <TableHead>Type</TableHead>
                    <TableHead className="text-right">Value</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {txs?.slice(0, 5).map((tx) => (
                    <TableRow key={tx.hash}>
                      <TableCell className="font-mono text-xs">
                        <Link href={`/txs/${tx.hash}`} className="text-primary hover:underline">
                          {formatAddress(tx.hash, 4)}
                        </Link>
                      </TableCell>
                      <TableCell className="font-mono text-xs text-muted-foreground">
                        <div>F: {formatAddress(tx.from, 4)}</div>
                        {tx.to && <div>T: {formatAddress(tx.to, 4)}</div>}
                      </TableCell>
                      <TableCell>
                        <Badge variant="outline" className="text-[10px] uppercase">
                          {tx.type}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-right font-mono text-xs">{formatNumber(tx.value, 4)} ZBX</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
